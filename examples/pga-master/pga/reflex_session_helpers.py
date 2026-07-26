#!/usr/bin/env python3
"""
Reflex-specific session recovery helpers.
These functions help handle DetachedInstanceError in Reflex state management.
"""

from typing import List, Optional, Any
import sqlalchemy.orm.exc


def safe_get_photo_sequences(mesh_model) -> List:
    """
    Safely get photo sequences from a mesh model, handling session expiration.
    
    Args:
        mesh_model: MeshModel instance (may be detached)
        
    Returns:
        List of PhotoSequence objects, or empty list if error
    """
    if not mesh_model or not hasattr(mesh_model, 'id') or not mesh_model.id:
        return []
    
    try:
        # First try direct access
        return mesh_model.photo_sequences
    except sqlalchemy.orm.exc.DetachedInstanceError:
        # Instance is detached, use safe relationship access
        try:
            sequences = mesh_model._safe_relationship_access('photo_sequences')
            return sequences if sequences else []
        except Exception as e:
            print(f"Error in safe relationship access: {e}")
            # Fallback: query directly from database
            return _fallback_get_photo_sequences(mesh_model.id)
    except Exception as e:
        print(f"Unexpected error accessing photo_sequences: {e}")
        return _fallback_get_photo_sequences(mesh_model.id)


def _fallback_get_photo_sequences(mesh_model_id) -> List:
    """
    Fallback method to get photo sequences by querying the database directly.
    
    Args:
        mesh_model_id: UUID of the mesh model
        
    Returns:
        List of PhotoSequence objects
    """
    try:
        from models.photo_sequence import PhotoSequence
        from models.database import safe_session_operation
        
        def query_sequences(session):
            return session.query(PhotoSequence).filter(
                PhotoSequence.mesh_model_id == mesh_model_id
            ).all()
        
        return safe_session_operation(query_sequences)
    except Exception as e:
        print(f"Error in fallback photo sequence query: {e}")
        return []


def refresh_mesh_model(mesh_model):
    """
    Refresh a mesh model instance from the database.
    
    Args:
        mesh_model: MeshModel instance to refresh
        
    Returns:
        Refreshed MeshModel instance or None if error
    """
    if not mesh_model or not hasattr(mesh_model, 'id') or not mesh_model.id:
        return None
    
    try:
        # Try to refresh the existing instance
        mesh_model.refresh_from_db()
        return mesh_model
    except Exception as e:
        print(f"Error refreshing mesh model: {e}")
        # Fallback: get fresh instance from database
        return _get_fresh_mesh_model(mesh_model.id)


def _get_fresh_mesh_model(mesh_model_id):
    """
    Get a fresh mesh model instance from the database.
    
    Args:
        mesh_model_id: UUID of the mesh model
        
    Returns:
        Fresh MeshModel instance or None if error
    """
    try:
        from models.mesh_model import MeshModel
        return MeshModel.find(mesh_model_id)
    except Exception as e:
        print(f"Error getting fresh mesh model: {e}")
        return None


def safe_count_photo_sequences(mesh_model) -> int:
    """
    Safely count photo sequences for a mesh model.
    
    Args:
        mesh_model: MeshModel instance
        
    Returns:
        Number of photo sequences
    """
    sequences = safe_get_photo_sequences(mesh_model)
    return len(sequences) if sequences else 0


def safe_access_relationship(instance, relationship_name: str, fallback_query_func=None):
    """
    Generic safe relationship access for any model instance.
    
    Args:
        instance: Model instance (may be detached)
        relationship_name: Name of the relationship attribute
        fallback_query_func: Optional function to query relationship directly
        
    Returns:
        Relationship value or None/empty list if error
    """
    if not instance:
        return None
    
    try:
        # First try direct access
        return getattr(instance, relationship_name)
    except sqlalchemy.orm.exc.DetachedInstanceError:
        # Instance is detached, try safe access
        try:
            if hasattr(instance, '_safe_relationship_access'):
                result = instance._safe_relationship_access(relationship_name)
                return result
            else:
                # Instance doesn't have safe access method, try fallback
                if fallback_query_func and hasattr(instance, 'id'):
                    return fallback_query_func(instance.id)
                return None
        except Exception as e:
            print(f"Error in safe relationship access for {relationship_name}: {e}")
            if fallback_query_func and hasattr(instance, 'id'):
                return fallback_query_func(instance.id)
            return None
    except Exception as e:
        print(f"Unexpected error accessing {relationship_name}: {e}")
        return None


# Reflex State Helper Functions
def reflex_safe_mesh_operations(state_instance):
    """
    Helper class for common mesh operations in Reflex state.
    
    Usage in your Reflex state:
        from reflex_session_helpers import reflex_safe_mesh_operations
        
        def load_photo_sequences(self):
            ops = reflex_safe_mesh_operations(self)
            sequences = ops.get_photo_sequences()
            print(f"Found {len(sequences)} photo sequences")
    """
    
    class MeshOperations:
        def __init__(self, state):
            self.state = state
        
        def get_photo_sequences(self):
            """Get photo sequences safely."""
            if not hasattr(self.state, 'selected_mesh_model') or not self.state.selected_mesh_model:
                return []
            return safe_get_photo_sequences(self.state.selected_mesh_model)
        
        def count_photo_sequences(self):
            """Count photo sequences safely."""
            if not hasattr(self.state, 'selected_mesh_model') or not self.state.selected_mesh_model:
                return 0
            return safe_count_photo_sequences(self.state.selected_mesh_model)
        
        def refresh_selected_mesh(self):
            """Refresh the selected mesh model."""
            if hasattr(self.state, 'selected_mesh_model') and self.state.selected_mesh_model:
                refreshed = refresh_mesh_model(self.state.selected_mesh_model)
                if refreshed:
                    self.state.selected_mesh_model = refreshed
                return refreshed
            return None
        
        def safe_access(self, relationship_name: str):
            """Safely access any relationship on the selected mesh."""
            if not hasattr(self.state, 'selected_mesh_model') or not self.state.selected_mesh_model:
                return None
            return safe_access_relationship(self.state.selected_mesh_model, relationship_name)
    
    return MeshOperations(state_instance)


# Example usage patterns for Reflex state methods
def example_reflex_state_methods():
    """
    Example of how to modify your Reflex state methods to use session recovery.
    """
    
    # Example 1: Replace direct photo_sequences access
    print("""
    # BEFORE (causes DetachedInstanceError):
    def load_photo_sequences(self):
        print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")
    
    # AFTER (safe):
    def load_photo_sequences(self):
        from reflex_session_helpers import safe_get_photo_sequences
        sequences = safe_get_photo_sequences(self.selected_mesh_model)
        print(f"Found {len(sequences)} photo sequences")
        return sequences
    """)
    
    # Example 2: Using the helper class
    print("""
    # Using the helper class:
    def load_photo_sequences(self):
        from reflex_session_helpers import reflex_safe_mesh_operations
        ops = reflex_safe_mesh_operations(self)
        sequences = ops.get_photo_sequences()
        print(f"Found {len(sequences)} photo sequences")
        return sequences
    """)
    
    # Example 3: Refresh before accessing relationships
    print("""
    # Refresh before accessing relationships:
    def copy_mesh_model_to_state(self, mesh_models):
        from reflex_session_helpers import refresh_mesh_model
        
        if self.selected_mesh_model:
            # Refresh the mesh model before accessing relationships
            refreshed_mesh = refresh_mesh_model(self.selected_mesh_model)
            if refreshed_mesh:
                self.selected_mesh_model = refreshed_mesh
        
        self.load_photo_sequences()
    """)


if __name__ == "__main__":
    example_reflex_state_methods()
