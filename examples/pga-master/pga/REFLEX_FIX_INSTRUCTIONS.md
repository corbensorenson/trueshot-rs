# Fix for Reflex DetachedInstanceError

## Problem
Your Reflex app is getting `DetachedInstanceError` when accessing `self.selected_mesh_model.photo_sequences` in the `load_photo_sequences` method.

## Root Cause
In Reflex, model instances stored in state become detached from their database sessions over time. When you try to access lazy-loaded relationships like `photo_sequences`, SQLAlchemy can't load them because the instance is no longer bound to a session.

## Solution

### Step 1: Copy the helper file
The helper file `pga/reflex_session_helpers.py` has been created with safe relationship access functions.

### Step 2: Modify your Reflex state code

In your Reflex state file (likely `indb_reflex/indb_reflex/photogrammetry/state.py`), make these changes:

#### BEFORE (line 711 - causing the error):
```python
def load_photo_sequences(self):
    print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")
    # ... rest of method
```

#### AFTER (safe version):
```python
def load_photo_sequences(self):
    # Import the helper function
    from pga.reflex_session_helpers import safe_get_photo_sequences
    
    # Use safe access instead of direct access
    sequences = safe_get_photo_sequences(self.selected_mesh_model)
    print(f"Found {len(sequences)} photo sequences")
    
    # Continue with your existing logic using 'sequences' instead of 'self.selected_mesh_model.photo_sequences'
    # ... rest of method
```

### Step 3: Also modify the calling method

In your `copy_mesh_model_to_state` method (around line 384), add a refresh before calling `load_photo_sequences`:

#### BEFORE:
```python
def copy_mesh_model_to_state(self, mesh_models):
    # ... existing code ...
    self.load_photo_sequences()
```

#### AFTER:
```python
def copy_mesh_model_to_state(self, mesh_models):
    # Import the helper function
    from pga.reflex_session_helpers import refresh_mesh_model
    
    # ... existing code ...
    
    # Refresh the mesh model before accessing relationships
    if self.selected_mesh_model:
        refreshed_mesh = refresh_mesh_model(self.selected_mesh_model)
        if refreshed_mesh:
            self.selected_mesh_model = refreshed_mesh
    
    self.load_photo_sequences()
```

## Alternative: Quick One-Line Fix

If you want a minimal change, just replace the problematic line:

#### BEFORE:
```python
print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")
```

#### AFTER:
```python
from pga.reflex_session_helpers import safe_count_photo_sequences
print(f"Found {safe_count_photo_sequences(self.selected_mesh_model)} photo sequences")
```

## Alternative: Using the Helper Class

For a more comprehensive solution, use the helper class:

```python
def load_photo_sequences(self):
    from pga.reflex_session_helpers import reflex_safe_mesh_operations
    
    ops = reflex_safe_mesh_operations(self)
    sequences = ops.get_photo_sequences()
    print(f"Found {len(sequences)} photo sequences")
    
    # Use 'sequences' for the rest of your logic
    # ... rest of method
```

## Testing the Fix

After making these changes:

1. Restart your Reflex app
2. Try the operation that was causing the error
3. The `DetachedInstanceError` should be resolved

## Additional Recommendations

1. **Apply this pattern to other relationship access**: If you have other places in your Reflex state where you access relationships directly, apply the same pattern.

2. **Refresh before bulk operations**: Before doing operations that access multiple relationships, refresh the model:
   ```python
   from pga.reflex_session_helpers import refresh_mesh_model
   self.selected_mesh_model = refresh_mesh_model(self.selected_mesh_model)
   ```

3. **Use safe access for all relationships**: Replace any direct relationship access with safe access:
   ```python
   # Instead of: mesh.photo_sequences
   # Use: safe_get_photo_sequences(mesh)
   ```

## Why This Works

1. **Safe fallback**: If the direct access fails, it automatically tries the `_safe_relationship_access` method
2. **Database fallback**: If that fails, it queries the database directly
3. **Error handling**: All operations are wrapped in try-catch blocks
4. **Session recovery**: Uses the session recovery features we implemented in INDBModel

This should completely resolve your `DetachedInstanceError` in the Reflex app.
