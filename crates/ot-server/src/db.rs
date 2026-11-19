//! Database access for the HTTP server: one writer thread and a pool of readers.
//!
//! DuckDB lets one process write to a book file. All writes run in order on a
//! dedicated thread that owns the migrated `Store`. Reads use cloned connections to
//! the same database instance, so a long report or a slow write does not block
//! other reads.

use std::sync::{Arc, Mutex};

use ot_store::{Store, StoreError};
use tokio::sync::{mpsc, oneshot};

type WriteJob = Box<dyn FnOnce(&mut Store) + Send>;

/// The most idle read connections kept for reuse.
const MAX_IDLE_READERS: usize = 8;

#[derive(Clone)]
pub struct Database {
    writer: mpsc::UnboundedSender<WriteJob>,
    readers: Arc<Readers>,
}

struct Readers {
    /// Source for new read connections; `Connection` is not `Sync`.
    origin: Mutex<Store>,
    idle: Mutex<Vec<Store>>,
}

impl Database {
    /// Starts the writer thread for an opened, migrated store.
    pub fn start(store: Store) -> Result<Self, StoreError> {
        let origin = store.reader()?;
        let (writer, mut jobs) = mpsc::unbounded_channel::<WriteJob>();
        std::thread::Builder::new()
            .name("opentally-writer".into())
            .spawn(move || {
                let mut store = store;
                while let Some(job) = jobs.blocking_recv() {
                    job(&mut store);
                }
            })
            .map_err(|_| StoreError::Unavailable)?;
        Ok(Self {
            writer,
            readers: Arc::new(Readers {
                origin: Mutex::new(origin),
                idle: Mutex::new(Vec::new()),
            }),
        })
    }

    /// Queues `work` on the writer thread now, so writes run in the order they are
    /// called, and returns a future for its result.
    pub fn write<T: Send + 'static>(
        &self,
        work: impl FnOnce(&mut Store) -> Result<T, StoreError> + Send + 'static,
    ) -> impl Future<Output = Result<T, StoreError>> + use<T> {
        let (reply, result) = oneshot::channel();
        let queued = self.writer.send(Box::new(move |store: &mut Store| {
            let _ = reply.send(work(store));
        }));
        async move {
            queued.map_err(|_| StoreError::Unavailable)?;
            result.await.map_err(|_| StoreError::Unavailable)?
        }
    }

    /// Runs read-only `work` on a pooled connection. It must not change the book.
    pub async fn read<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Store) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let readers = Arc::clone(&self.readers);
        tokio::task::spawn_blocking(move || {
            let store = readers.checkout()?;
            let result = work(&store);
            readers.checkin(store);
            result
        })
        .await
        .map_err(|error| {
            eprintln!("Database reader error: {error}");
            StoreError::Unavailable
        })?
    }
}

impl Readers {
    fn checkout(&self) -> Result<Store, StoreError> {
        if let Some(store) = self.idle.lock().map_err(|_| StoreError::Unavailable)?.pop() {
            return Ok(store);
        }
        self.origin
            .lock()
            .map_err(|_| StoreError::Unavailable)?
            .reader()
    }

    fn checkin(&self, store: Store) {
        if let Ok(mut idle) = self.idle.lock() {
            if idle.len() < MAX_IDLE_READERS {
                idle.push(store);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn database() -> Database {
        Database::start(Store::open_in_memory().unwrap()).unwrap()
    }

    #[tokio::test]
    async fn reads_see_committed_writes() {
        let db = database();
        let name = serde_json::from_value(serde_json::json!({"name": "Shree Traders"})).unwrap();
        db.write(move |store| store.name_company(name)).await.unwrap();
        let settings = db.read(|store| store.settings()).await.unwrap();
        assert_eq!(
            serde_json::to_value(settings).unwrap()["company_name"],
            "Shree Traders"
        );
    }

    #[tokio::test]
    async fn reads_are_not_blocked_by_a_running_write() {
        let db = database();
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (started, running) = oneshot::channel();
        let slow_write = {
            let db = db.clone();
            tokio::spawn(async move {
                db.write(move |_store| {
                    let _ = started.send(());
                    held.recv().ok();
                    Ok(())
                })
                .await
            })
        };
        running.await.unwrap();

        let read = tokio::time::timeout(
            Duration::from_secs(10),
            db.read(|store| store.ledgers()),
        )
        .await
        .expect("a read finished while the writer was busy")
        .unwrap();
        assert!(read.len() >= 2, "a new book has Cash and Bank");

        release.send(()).unwrap();
        slow_write.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn writes_run_in_order() {
        let db = database();
        let log = Arc::new(Mutex::new(Vec::new()));
        // Futures are awaited only after all 20 are queued, in reverse.
        let mut pending = Vec::new();
        for n in 0..20 {
            let log = Arc::clone(&log);
            pending.push(db.write(move |_store| {
                log.lock().unwrap().push(n);
                Ok(())
            }));
        }
        for job in pending.into_iter().rev() {
            job.await.unwrap();
        }
        assert_eq!(*log.lock().unwrap(), (0..20).collect::<Vec<_>>());
    }
}
