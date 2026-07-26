//! Representation Transitions
//!
//! Manages smooth transitions when objects change representation:
//! - 4DGS → Mesh (meshification complete)
//! - Mesh → 4DGS (object started moving)
//! - Avatar binding/unbinding

use std::collections::HashMap;
use std::time::{Duration, Instant};
use uuid::Uuid;

use super::scene_graph::ObjectRepresentation;

/// A transition between representations
#[derive(Clone, Debug)]
pub struct Transition {
    /// Object being transitioned
    pub object_id: Uuid,
    /// Source representation
    pub from: Box<ObjectRepresentation>,
    /// Target representation
    pub to: Box<ObjectRepresentation>,
    /// When transition started
    pub start_time: Instant,
    /// Transition duration
    pub duration: Duration,
    /// Easing function type
    pub easing: EasingFunction,
}

impl Transition {
    pub fn new(
        object_id: Uuid,
        from: ObjectRepresentation,
        to: ObjectRepresentation,
        duration: Duration,
    ) -> Self {
        Self {
            object_id,
            from: Box::new(from),
            to: Box::new(to),
            start_time: Instant::now(),
            duration,
            easing: EasingFunction::EaseInOut,
        }
    }
    
    /// Get normalized progress (0.0 to 1.0)
    pub fn progress(&self) -> f32 {
        let elapsed = self.start_time.elapsed();
        let raw = elapsed.as_secs_f32() / self.duration.as_secs_f32();
        let clamped = raw.clamp(0.0, 1.0);
        self.easing.apply(clamped)
    }
    
    /// Check if transition is complete
    pub fn is_complete(&self) -> bool {
        self.start_time.elapsed() >= self.duration
    }
}

/// Easing functions for smooth transitions
#[derive(Clone, Copy, Debug)]
pub enum EasingFunction {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl EasingFunction {
    pub fn apply(&self, t: f32) -> f32 {
        match self {
            EasingFunction::Linear => t,
            EasingFunction::EaseIn => t * t,
            EasingFunction::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            EasingFunction::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
                }
            }
        }
    }
}

/// Manages active transitions
pub struct TransitionManager {
    /// Active transitions by object ID
    active: HashMap<Uuid, Transition>,
    /// Default transition duration
    default_duration: Duration,
}

impl Default for TransitionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TransitionManager {
    pub fn new() -> Self {
        Self {
            active: HashMap::new(),
            default_duration: Duration::from_millis(500),
        }
    }
    
    pub fn with_duration(duration: Duration) -> Self {
        Self {
            active: HashMap::new(),
            default_duration: duration,
        }
    }
    
    /// Start a new transition
    pub fn start_transition(
        &mut self,
        object_id: Uuid,
        from: ObjectRepresentation,
        to: ObjectRepresentation,
    ) {
        let transition = Transition::new(object_id, from, to, self.default_duration);
        self.active.insert(object_id, transition);
    }
    
    /// Start transition with custom duration
    pub fn start_transition_with_duration(
        &mut self,
        object_id: Uuid,
        from: ObjectRepresentation,
        to: ObjectRepresentation,
        duration: Duration,
    ) {
        let transition = Transition::new(object_id, from, to, duration);
        self.active.insert(object_id, transition);
    }
    
    /// Get transition progress for an object
    pub fn get_progress(&self, object_id: Uuid) -> Option<f32> {
        self.active.get(&object_id).map(|t| t.progress())
    }
    
    /// Get a transition
    pub fn get_transition(&self, object_id: Uuid) -> Option<&Transition> {
        self.active.get(&object_id)
    }
    
    /// Check if object is transitioning
    pub fn is_transitioning(&self, object_id: Uuid) -> bool {
        self.active.contains_key(&object_id)
    }
    
    /// Update and clean up completed transitions
    /// Returns list of completed object IDs with their final representations
    pub fn update(&mut self) -> Vec<(Uuid, ObjectRepresentation)> {
        let mut completed = Vec::new();
        
        self.active.retain(|id, transition| {
            if transition.is_complete() {
                completed.push((*id, (*transition.to).clone()));
                false
            } else {
                true
            }
        });
        
        completed
    }
    
    /// Cancel a transition
    pub fn cancel(&mut self, object_id: Uuid) -> Option<Transition> {
        self.active.remove(&object_id)
    }
    
    /// Get number of active transitions
    pub fn active_count(&self) -> usize {
        self.active.len()
    }
}

/// Blend parameters for rendering during transition
#[derive(Clone, Debug)]
pub struct TransitionBlend {
    /// Opacity of "from" representation (decreasing)
    pub from_opacity: f32,
    /// Opacity of "to" representation (increasing)
    pub to_opacity: f32,
    /// Scale adjustment for smooth size matching
    pub scale_blend: f32,
}

impl TransitionBlend {
    pub fn from_progress(progress: f32) -> Self {
        Self {
            from_opacity: 1.0 - progress,
            to_opacity: progress,
            scale_blend: progress,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_easing_functions() {
        // Linear
        assert_eq!(EasingFunction::Linear.apply(0.0), 0.0);
        assert_eq!(EasingFunction::Linear.apply(0.5), 0.5);
        assert_eq!(EasingFunction::Linear.apply(1.0), 1.0);
        
        // EaseIn starts slow
        assert!(EasingFunction::EaseIn.apply(0.5) < 0.5);
        
        // EaseOut ends slow
        assert!(EasingFunction::EaseOut.apply(0.5) > 0.5);
    }
    
    #[test]
    fn test_transition_manager() {
        let mut manager = TransitionManager::new();
        let id = Uuid::new_v4();
        
        manager.start_transition(
            id,
            ObjectRepresentation::Pending,
            ObjectRepresentation::Pending,
        );
        
        assert!(manager.is_transitioning(id));
        assert_eq!(manager.active_count(), 1);
    }
}
