use crate::config::Config;
use sqlx::PgPool;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::{Arc, PoisonError},
};
use tokio::sync::Semaphore;

/// What every request and background task of the panel shares: the database, the
/// configuration, process-wide budgets, the compiled release trust root and the runtime
/// state each domain keeps for itself.
#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub started_at: i64,
    pub login_permits: Arc<Semaphore>,
    pub quality_permits: Arc<Semaphore>,
    pub release_permits: Arc<Semaphore>,
    pub release_keys: Option<Arc<apsp_protocol::release::TrustedKeys>>,
    pub config: Arc<Config>,
    pub domain_states: DomainStates,
}

impl AppState {
    /// Checks the database baseline and builds the shared state. It prepares nothing that
    /// belongs to a domain; the application's startup does that.
    pub async fn new(pool: PgPool, config: Config) -> anyhow::Result<Self> {
        crate::schema::initialize(&pool).await?;
        Ok(Self {
            pool,
            started_at: apsp_protocol::now_timestamp(),
            login_permits: Arc::new(Semaphore::new(4)),
            quality_permits: Arc::new(Semaphore::new(2)),
            release_permits: Arc::new(Semaphore::new(1)),
            release_keys: apsp_protocol::release::TrustedKeys::compiled()
                .ok()
                .map(Arc::new),
            config: Arc::new(config),
            domain_states: DomainStates::default(),
        })
    }
}

type Slots = HashMap<TypeId, Arc<dyn Any + Send + Sync>>;

/// Runtime state that a domain keeps for the whole process, such as live caches and
/// provider registries: one value per type. The platform knows none of these types;
/// a domain keeps its type private, creates the value on first use, and every copy of
/// the application state sees the same value.
#[derive(Clone, Default)]
pub struct DomainStates(Arc<std::sync::Mutex<Slots>>);

impl DomainStates {
    /// The value for `T`, created by `init` the first time any copy of the state asks.
    pub fn get_or_init<T: Send + Sync + 'static>(&self, init: impl FnOnce() -> T) -> Arc<T> {
        if let Some(value) = self.get::<T>() {
            return value;
        }
        let created: Arc<dyn Any + Send + Sync> = Arc::new(init());
        let value = Arc::clone(self.slots().entry(TypeId::of::<T>()).or_insert(created));
        downcast(value)
    }

    /// A copy whose value for `T` is `value`, for tests that substitute one; copies of
    /// the state made before keep theirs.
    pub fn with<T: Send + Sync + 'static>(&self, value: T) -> Self {
        let mut slots = self.slots().clone();
        slots.insert(TypeId::of::<T>(), Arc::new(value));
        Self(Arc::new(std::sync::Mutex::new(slots)))
    }

    fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.slots().get(&TypeId::of::<T>()).cloned().map(downcast)
    }

    fn slots(&self) -> std::sync::MutexGuard<'_, Slots> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

fn downcast<T: Send + Sync + 'static>(value: Arc<dyn Any + Send + Sync>) -> Arc<T> {
    value
        .downcast::<T>()
        .unwrap_or_else(|_| unreachable!("a slot holds the type it is keyed by"))
}

#[cfg(test)]
mod tests {
    use super::DomainStates;

    #[derive(Default)]
    struct Counter(std::sync::atomic::AtomicUsize);

    #[test]
    fn values_are_created_once_shared_by_copies_and_replaced_only_in_new_copies() {
        let states = DomainStates::default();
        let copy = states.clone();
        let first = states.get_or_init(Counter::default);
        first.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let seen = copy.get_or_init(|| Counter(std::sync::atomic::AtomicUsize::new(7)));
        assert_eq!(seen.0.load(std::sync::atomic::Ordering::SeqCst), 1);
        let replaced = states.with(Counter(std::sync::atomic::AtomicUsize::new(5)));
        assert_eq!(
            replaced
                .get_or_init(Counter::default)
                .0
                .load(std::sync::atomic::Ordering::SeqCst),
            5
        );
        assert_eq!(
            states
                .get_or_init(Counter::default)
                .0
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }
}
