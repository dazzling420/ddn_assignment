//! Bounded Async Worker Pool
//!
//! A pool that dynamically accepts jobs, executing at most `max_concurrent` concurrently.
//! It buffers up to `queue_capacity` jobs, applying backpressure to writers once filled.
//! It supports a graceful shutdown where the pool stops accepting jobs but ensures all
//! queued and active jobs complete before exiting.

use futures::future::BoxFuture;
use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{Semaphore, mpsc};
use tokio_util::task::TaskTracker;

/// Tracks counts of active, queued, and completed jobs.
#[derive(Debug, Default)]
pub struct Metrics {
    pub active_jobs: AtomicUsize,
    pub queued_jobs: AtomicUsize,
    pub completed_jobs: AtomicUsize,
}

pub struct WorkerPool {
    sender: Option<mpsc::Sender<BoxFuture<'static, ()>>>,
    tracker: Box<TaskTracker>,
    metrics: Arc<Metrics>,
    dispatcher_handle: tokio::task::JoinHandle<()>,
}

impl WorkerPool {
    /// Creates a new worker pool.
    ///
    /// * `max_concurrent` is the maximum number of jobs running concurrently.
    /// * `queue_capacity` represents how many jobs can be queued up before backpressure blocks `submit`.
    pub fn new(max_concurrent: usize, queue_capacity: usize) -> Self {
        let (tx, mut rx) = mpsc::channel::<BoxFuture<'static, ()>>(queue_capacity);
        let semaphore = Arc::new(Semaphore::new(max_concurrent));
        let tracker = Box::new(TaskTracker::new());
        let metrics = Arc::new(Metrics::default());

        let metrics_cloned = metrics.clone();
        let tracker_cloned = tracker.clone();

        // The dispatcher pulls jobs from the queue and spawns them when concurrency allows.
        let dispatcher_handle = tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                // A job has left the queue and is about to wait for concurrency limits
                metrics_cloned.queued_jobs.fetch_sub(1, Ordering::Relaxed);

                // Acquire a permit. This limits concurrency. If max_concurrent is reached,
                // the dispatcher blocks here. This prevents pulling from the channel, which
                // fills the channel, giving backpressure to the submitter.
                let permit = semaphore
                    .clone()
                    .acquire_owned()
                    .await
                    .expect("Semaphore should never be closed under normal operation");

                metrics_cloned.active_jobs.fetch_add(1, Ordering::Relaxed);
                let metrics_task = metrics_cloned.clone();

                // Spawn the actual job into the TaskTracker
                tracker_cloned.spawn(async move {
                    job.await;

                    metrics_task.active_jobs.fetch_sub(1, Ordering::Relaxed);
                    metrics_task.completed_jobs.fetch_add(1, Ordering::Relaxed);
                    // The permit is released here, freeing a concurrency slot for the dispatcher
                    drop(permit);
                });
            }
        });

        Self {
            sender: Some(tx),
            tracker,
            metrics,
            dispatcher_handle,
        }
    }

    /// Submits a task to the pool.
    ///
    /// If the queue is at capacity, this method will asynchronously block and yield
    /// backpressure until space is available.
    pub async fn submit<F>(&self, job: F)
    where
        F: Future<Output = ()> + Send + 'static,
    {
        if let Some(sender) = &self.sender {
            self.metrics.queued_jobs.fetch_add(1, Ordering::Relaxed);
            if sender.send(Box::pin(job)).await.is_err() {
                // Job failed to queue, decrement metric
                self.metrics.queued_jobs.fetch_sub(1, Ordering::Relaxed);
            }
        }
    }

    /// Gets the current pool metrics.
    pub fn metrics(&self) -> Arc<Metrics> {
        self.metrics.clone()
    }

    /// Gracefully shuts down the worker pool.
    ///
    /// Existing queued and active tasks will finish. No new tasks can be submitted (if there were other owners of the pool).
    pub async fn shutdown(mut self) {
        // Drop the channel sender. This closes the channel so rx.recv() loop will terminate
        // after draining the existing queue.
        self.sender.take();

        // Wait for the dispatcher to drain the channel and spawn all tasks
        let _ = self.dispatcher_handle.await;

        // Ensure tracker rejects new tasks, though logically none should arrive now
        self.tracker.close();

        // Wait until all tracked tasks have completed
        self.tracker.wait().await;
    }
}
