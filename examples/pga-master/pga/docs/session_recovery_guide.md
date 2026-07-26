# Session Recovery Guide for Neon Database

This guide explains the session recovery features implemented in `INDBModel` to handle Neon database connection timeouts and session expiration issues.

## Problem Description

When using Neon cloud database, connections can expire quickly (sometimes < 1 minute), leading to `DetachedInstanceError` when trying to access lazy-loaded relationships:

```
sqlalchemy.orm.exc.DetachedInstanceError: Parent instance <MeshModel at 0x1235d9a90> is not bound to a Session; lazy load operation of attribute 'photo_sequences' cannot proceed
```

## Solution Overview

The enhanced `INDBModel` now includes:

1. **Connection pooling** with pre-ping validation
2. **Automatic session recovery** for expired sessions
3. **Safe relationship access** methods
4. **Retry logic** for connection failures
5. **Session validation** utilities

## Key Features

### 1. Enhanced Database Configuration

The `database.py` file now includes:

- Connection pooling with `pool_pre_ping=True`
- Connection recycling every 5 minutes
- Timeout configurations for Neon
- Connection event logging

### 2. Session Recovery Methods

#### `_get_session_for_instance(session=None)`
Returns a valid session, creating a new one if the provided session is expired.

#### `_reattach_to_session(session)`
Reattaches a detached instance to a session using `session.merge()`.

#### `_safe_relationship_access(relationship_name, session=None)`
Safely accesses relationship attributes, handling session expiration automatically.

#### `refresh_from_db(session=None)`
Refreshes the instance from the database with automatic session recovery.

### 3. Enhanced CRUD Operations

All CRUD operations now include:
- Automatic session validation
- Retry logic for connection failures
- Proper session cleanup

## Usage Examples

### Basic Usage (No Changes Required)

Your existing code continues to work without changes:

```python
# This still works as before, but now with automatic session recovery
mesh = MeshModel.find("some-id")
sequences = mesh.photo_sequences  # Automatically handles session expiration
```

### Safe Relationship Access

For critical operations, use the safe access method:

```python
mesh = MeshModel.find("some-id")
# This will handle session expiration gracefully
sequences = mesh._safe_relationship_access('photo_sequences')
```

### Manual Session Management

When you need more control:

```python
session = Session()
try:
    mesh = MeshModel.find("some-id", session)
    # Do multiple operations with the same session
    mesh.name = "Updated Name"
    mesh.save(session)
    
    # Access relationships safely
    sequences = mesh._safe_relationship_access('photo_sequences', session)
finally:
    session.close()
```

### Using Safe Operations

For custom database operations:

```python
from models.database import safe_session_operation

def custom_query(session):
    return session.query(MeshModel).filter(MeshModel.name.like('%pattern%')).all()

# This handles session expiration automatically
results = safe_session_operation(custom_query)
```

## Best Practices

### 1. Let the System Handle Sessions

For most operations, let the enhanced methods handle session management:

```python
# Good - automatic session management
mesh = MeshModel.find("some-id")
mesh.save()

# Also good - explicit session passing
session = Session()
mesh = MeshModel.find("some-id", session)
mesh.save(session)
session.close()
```

### 2. Use Safe Relationship Access for Critical Code

```python
# For critical relationship access
sequences = mesh._safe_relationship_access('photo_sequences')

# For bulk operations, refresh the instance
mesh.refresh_from_db()
sequences = mesh.photo_sequences
```

### 3. Handle Long-Running Operations

For operations that might take longer than the connection timeout:

```python
def long_running_operation():
    mesh = MeshModel.find("some-id")
    
    # Do some work...
    time.sleep(300)  # 5 minutes
    
    # Refresh before accessing relationships
    mesh.refresh_from_db()
    sequences = mesh.photo_sequences
```

## Configuration Options

### Database Connection Settings

In `database.py`, you can adjust:

```python
engine = create_engine(
    "your-connection-string",
    pool_size=5,           # Number of connections to maintain
    max_overflow=10,       # Additional connections when needed
    pool_recycle=300,      # Recycle connections every 5 minutes
    pool_pre_ping=True,    # Validate connections before use
    connect_args={
        "connect_timeout": 10,  # Connection timeout
        "application_name": "your_app"
    }
)
```

## Troubleshooting

### Common Issues and Solutions

1. **Still getting DetachedInstanceError**
   - Use `_safe_relationship_access()` method
   - Call `refresh_from_db()` before accessing relationships

2. **Connection timeouts**
   - Reduce `pool_recycle` time
   - Increase `connect_timeout`
   - Use shorter-lived sessions

3. **Performance issues**
   - Adjust `pool_size` and `max_overflow`
   - Use eager loading for frequently accessed relationships

### Debugging

Enable logging to see connection events:

```python
import logging
logging.basicConfig(level=logging.DEBUG)
```

## Migration Guide

### Existing Code

Most existing code will work without changes. However, for better reliability:

1. Replace direct relationship access in critical code:
   ```python
   # Before
   sequences = mesh.photo_sequences
   
   # After (more reliable)
   sequences = mesh._safe_relationship_access('photo_sequences')
   ```

2. Add refresh calls for long-running operations:
   ```python
   # Before
   # ... long operation ...
   sequences = mesh.photo_sequences
   
   # After
   # ... long operation ...
   mesh.refresh_from_db()
   sequences = mesh.photo_sequences
   ```

3. Use the new safe operations for custom queries:
   ```python
   # Before
   session = Session()
   results = session.query(Model).filter(...).all()
   session.close()
   
   # After
   def query_op(session):
       return session.query(Model).filter(...).all()
   results = safe_session_operation(query_op)
   ```

## Testing

Run the example script to test the session recovery features:

```bash
python examples/session_recovery_example.py
```

This will demonstrate the session recovery capabilities and help verify that everything is working correctly.
