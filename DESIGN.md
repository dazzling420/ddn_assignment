# Bounded Async Worker Pool Design

## Architecture and Concurrency Model

The architecture is built upon the **Dispatcher pattern** leveraging `tokio` channels, semaphores, and a task tracker.

1. **Job Queueing**: 
   - A `tokio::sync::mpsc::channel` holds the incoming job futures. This sets the **queue capacity** bound.
2. **Execution Bounding**: 
   - A `tokio::sync::Semaphore` controls maximum concurrent jobs.
3. **Dispatcher Loop**: 
   - A single asynchronous background dispatcher `tokio::spawn` loop manages the interaction.
   - It awaits incoming jobs from the `mpsc::Receiver`.
   - Before executing a job, it strictly awaits `semaphore.acquire_owned()`.
   - Once acquired, it spawns the user's Future into a `TaskTracker` to execute concurrently, ensuring non-blocking operations.
   - The job completes, the semaphore permit is automatically dropped, making room for another task.

## Implementation of Backpressure

Backpressure is achieved naturally through the interaction between the `mpsc::channel` boundary and the dispatcher's concurrency permit checks:

- If the system is executing the `max_concurrent` jobs, the Semaphore hits 0 permits.
- The Dispatcher loop calls `acquire_owned().await`, which blocks the loop.
- Because the loop is blocked, it stops reading from the `mpsc` queue.
- If more jobs are submitted, the `mpsc` queue will fill up up to `queue_capacity`.
- Finally, when the `mpsc` queue is full, further calls to `pool.submit(job).await` will naturally block, providing direct, non-busy-waiting backpressure back to the caller.

## Graceful Shutdown

Graceful shutdown ensures no jobs in-flight or waiting in the queue are dropped.

- The `shutdown()` method consumes the `WorkerPool` so no more jobs can be queued.
- It drops `self.sender`, the MPSC channel sender. 
- The `mpsc::Receiver` in the dispatcher loop processes the remaining queued jobs. Once empty, `recv().await` returns `None`, breaking the dispatcher loop.
- `shutdown()` awaits the `dispatcher_handle`, ensuring all queued jobs have been dispatched into the `TaskTracker`.
- Finally, `TaskTracker::close()` and `TaskTracker::wait().await` is called, asynchronously waiting for all actively executing futures to finish.

## Trade-Offs

**Simplicity vs. Performance**: 
- **Pro**: Relying on Tokio’s mature components (`mpsc`, `Semaphore`, `TaskTracker`) offloads edge-case handling, leading to a much safer, concise and bug-resistant solution.
- **Con**: We use `BoxFuture<'static, ()>`, bringing small heap allocation overhead per job. In an extremely tight loop of micro-tasks, this box overhead could be noticeable compared to highly optimized lock-free custom schedulers, though it's standard for async ecosystem traits.

**Control vs. Correctness**:
- Tokio handles the `Send + 'static` execution across OS threads automatically underneath. Instead of spinning up 1:1 OS worker threads (which breaks the non-blocking trait requirement of modern async design), this uses "green" tokio tasks, allowing massive scaling.

## Limitations

- The pool is statically sized at instantiation. If dynamic resizing of max concurrent jobs was implemented, it would require extending the `Semaphore` permits using `Semaphore::add_permits()`, but shrinking the pool dynamically is trickier without more complex lock logic.
- Job futures must borrow owned data or `Arc` since they require the `'static` lifetime bound.
