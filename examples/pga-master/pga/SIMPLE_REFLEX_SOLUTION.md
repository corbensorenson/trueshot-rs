# Simple Solution for Reflex DetachedInstanceError

## The Problem
Your Reflex app is getting `DetachedInstanceError` when accessing `self.selected_mesh_model.photo_sequences`.

## The Simple Solution

I've added a `safe_getattr` method to `INDBModel` that you can use in your Reflex state. Here are three ways to fix your issue:

### Option 1: Use safe_getattr (Recommended)

In your Reflex state file, change this line:

```python
# BEFORE (line 711 - causing error):
print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")

# AFTER (safe version):
sequences = self.selected_mesh_model.safe_getattr('photo_sequences', [])
print(f"Found {len(sequences)} photo sequences")
```

### Option 2: Use _safe_relationship_access

```python
# BEFORE:
print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")

# AFTER:
sequences = self.selected_mesh_model._safe_relationship_access('photo_sequences')
sequences = sequences if sequences else []
print(f"Found {len(sequences)} photo sequences")
```

### Option 3: Use try-catch with refresh

```python
# BEFORE:
print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")

# AFTER:
try:
    sequences = self.selected_mesh_model.photo_sequences
except DetachedInstanceError:
    self.selected_mesh_model.refresh_from_db()
    sequences = self.selected_mesh_model.photo_sequences
print(f"Found {len(sequences)} photo sequences")
```

## Complete Example for Your Reflex State

Here's how to modify your `load_photo_sequences` method:

```python
def load_photo_sequences(self):
    """Load photo sequences safely handling session expiration."""
    
    if not self.selected_mesh_model:
        return
    
    # Use safe_getattr to handle DetachedInstanceError automatically
    sequences = self.selected_mesh_model.safe_getattr('photo_sequences', [])
    print(f"Found {len(sequences)} photo sequences")
    
    # Continue with your existing logic using 'sequences'
    # ... rest of your method
```

## Why This Works

1. **safe_getattr** tries normal attribute access first
2. If it gets `DetachedInstanceError`, it automatically uses `_safe_relationship_access`
3. If that fails, it returns the default value you specify
4. No complex automatic interception that could cause recursion

## Benefits

- ✅ **Simple**: Just change one line in your Reflex code
- ✅ **Safe**: No recursion issues or complex overrides
- ✅ **Explicit**: You control when to use safe access
- ✅ **Flexible**: You can specify default values
- ✅ **Backward compatible**: Doesn't affect other code

## Quick Fix for Your Immediate Issue

Replace this line in your Reflex state (around line 711):

```python
print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")
```

With this:

```python
sequences = self.selected_mesh_model.safe_getattr('photo_sequences', [])
print(f"Found {len(sequences)} photo sequences")
```

That's it! Your Reflex app should now work without the `DetachedInstanceError`.

## For Other Relationships

You can use the same pattern for any relationship access in Reflex:

```python
# Instead of: mesh.photo_sequences
sequences = mesh.safe_getattr('photo_sequences', [])

# Instead of: sequence.mesh_model  
mesh = sequence.safe_getattr('mesh_model', None)

# Instead of: machine.jobs
jobs = machine.safe_getattr('jobs', [])
```

This approach is much simpler and safer than trying to automatically intercept all attribute access.
