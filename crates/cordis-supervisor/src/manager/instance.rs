//! Instance lifecycle is a normal kernel service in an instance-owned realm.
//!
//! The service value identifies the committed remote generation. Consumers use
//! the kernel's dependency and effect ordering, including while a provider is
//! being withdrawn; the supervisor does not maintain another readiness state.

use super::*;
use cordis::ServiceKey;

/// Core assembly dependency on one remote instance's current loading generation.
///
/// Use [`Manager::instance_lifecycle_view`] to select the instance, then declare
/// this key in the component's `Inject`. This internal key is not a UP interface.
pub struct InstanceLifecycle;

impl ServiceKey for InstanceLifecycle {
    const NAME: &'static str = "cordis.instance";
    type Value = InstanceGeneration;
    type Meta = ();
}

/// Immutable identity read from a component's committed lifecycle binding.
///
/// The weak generation link cannot keep a retired remote context alive. An old
/// value continues to identify its old generation and never resolves by name.
pub struct InstanceGeneration {
    generation: Weak<Generation>,
}

impl InstanceGeneration {
    pub(super) fn new(generation: &Arc<Generation>) -> Self {
        Self {
            generation: Arc::downgrade(generation),
        }
    }

    /// Component name supplied by this generation's validated descriptor.
    pub fn component(&self) -> Result<String> {
        self.generation
            .upgrade()
            .map(|generation| generation.component.info.name.clone())
            .ok_or_else(|| error(4, "父实例加载纪元已退役"))
    }

    /// The exact generation's parent view, with kernel-owned epoch admission.
    pub fn context(&self) -> Result<Context> {
        let generation = self
            .generation
            .upgrade()
            .ok_or_else(|| error(4, "父实例加载纪元已退役"))?;
        generation.active()?;
        Ok(generation.context.clone())
    }
}
