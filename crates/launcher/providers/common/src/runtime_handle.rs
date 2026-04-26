use futures::future::BoxFuture;

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

type BoxAny = Box<dyn Any + Send>;
type BlockingJob = Box<dyn FnOnce() -> BoxAny + Send>;
type BlockingResultFuture = Pin<Box<dyn Future<Output = anyhow::Result<BoxAny>> + Send>>;

#[derive(Clone)]
pub struct RuntimeHandle {
	spawn_fn: Arc<dyn Fn(BoxFuture<'static, ()>) + Send + Sync>,
	spawn_blocking_fn: Arc<dyn Fn(BlockingJob) -> BlockingResultFuture + Send + Sync>,
}

impl RuntimeHandle {
	pub fn new(
		spawn_fn: impl Fn(BoxFuture<'static, ()>) + Send + Sync + 'static,
		spawn_blocking_fn: impl Fn(BlockingJob) -> BlockingResultFuture + Send + Sync + 'static,
	) -> Self {
		Self {
			spawn_fn: Arc::new(spawn_fn),
			spawn_blocking_fn: Arc::new(spawn_blocking_fn),
		}
	}
	pub fn spawn(&self, fut: BoxFuture<'static, ()>) {
		(self.spawn_fn)(fut);
	}
	pub async fn spawn_blocking<T, F>(&self, job: F) -> anyhow::Result<T>
	where
		T: Send + 'static,
		F: FnOnce() -> T + Send + 'static,
	{
		let erased_job: BlockingJob = Box::new(move || Box::new(job()) as BoxAny);
		let erased = (self.spawn_blocking_fn)(erased_job).await?;
		erased
			.downcast::<T>()
			.map(|boxed| *boxed)
			.map_err(|_| anyhow::anyhow!("spawn_blocking type mismatch"))
	}
}
