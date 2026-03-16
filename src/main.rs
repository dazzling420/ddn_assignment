use bounded_async_worker_pool::WorkerPool;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[tokio::main]
async fn main() {
    println!("Starting Worker Pool Demonstration...");
    // Pool allowing 3 concurrent jobs and a queue capacity of 5.
    let pool = WorkerPool::new(3, 5);
    let metrics = pool.metrics();

    println!("Submitting jobs...");
    for i in 1..=10 {
        let metrics_clone = metrics.clone();
        pool.submit(async move {
            println!(
                "Job {} started. [Active: {}, Queued: {}]",
                i,
                metrics_clone.active_jobs.load(Ordering::Relaxed),
                metrics_clone.queued_jobs.load(Ordering::Relaxed)
            );
            tokio::time::sleep(Duration::from_millis(500)).await;
            println!("Job {} finished.", i);
        })
        .await;
        println!("Job {} submitted to the pool.", i);
    }

    println!("\nAll jobs submitted. Initiating graceful shutdown...");
    pool.shutdown().await;

    println!("\nShutdown complete. Metrics at completion:");
    println!("Active: {}", metrics.active_jobs.load(Ordering::Relaxed));
    println!("Queued: {}", metrics.queued_jobs.load(Ordering::Relaxed));
    println!(
        "Completed: {}",
        metrics.completed_jobs.load(Ordering::Relaxed)
    );
}
