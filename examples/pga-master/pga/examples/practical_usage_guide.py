#!/usr/bin/env python3
"""
Practical usage guide for session recovery in your existing models.
This shows how to use the new session recovery features to prevent DetachedInstanceError.
"""

def example_usage_patterns():
    """
    Examples of how to use the session recovery features in your existing code.
    These are code patterns you can apply to your actual application.
    """
    
    print("=== Practical Usage Patterns for Session Recovery ===\n")
    
    print("1. BASIC USAGE (No changes needed)")
    print("   Your existing code continues to work, but now with automatic session recovery:")
    print("""
   # This code pattern continues to work as before, but now handles session expiration:
   mesh = MeshModel.find("some-id")
   mesh.name = "Updated Name"
   mesh.save()  # Now includes automatic session recovery
   """)
    
    print("\n2. SAFE RELATIONSHIP ACCESS")
    print("   For critical relationship access, use the safe method:")
    print("""
   # Instead of:
   # sequences = mesh.photo_sequences  # Might fail with DetachedInstanceError
   
   # Use:
   sequences = mesh._safe_relationship_access('photo_sequences')
   if sequences:
       for seq in sequences:
           print(f"Sequence: {seq.description}")
   """)
    
    print("\n3. LONG-RUNNING OPERATIONS")
    print("   For operations that might exceed connection timeout:")
    print("""
   def long_running_photo_processing(mesh_id):
       mesh = MeshModel.find(mesh_id)
       
       # Do some long processing...
       process_photos()  # This might take several minutes
       
       # Refresh the instance before accessing relationships
       mesh.refresh_from_db()
       sequences = mesh.photo_sequences  # Now safe to access
       
       return sequences
   """)
    
    print("\n4. BULK OPERATIONS")
    print("   For processing multiple objects:")
    print("""
   def process_all_meshes():
       meshes = MeshModel.all()
       
       for mesh in meshes:
           # Refresh each mesh before processing to ensure valid session
           mesh.refresh_from_db()
           
           # Process the mesh
           sequences = mesh._safe_relationship_access('photo_sequences')
           if sequences:
               for seq in sequences:
                   # Process each sequence
                   seq.status = "processed"
                   seq.save()  # Includes automatic session recovery
   """)
    
    print("\n5. ERROR HANDLING")
    print("   How to handle remaining edge cases:")
    print("""
   def robust_relationship_access(mesh):
       try:
           # Try normal access first
           return mesh.photo_sequences
       except DetachedInstanceError:
           # Fallback to safe access
           print("Session expired, using safe access...")
           return mesh._safe_relationship_access('photo_sequences')
       except Exception as e:
           print(f"Unexpected error: {e}")
           return []
   """)
    
    print("\n6. CUSTOM SESSION MANAGEMENT")
    print("   When you need more control over sessions:")
    print("""
   from models.database import Session, safe_session_operation
   
   def custom_operation():
       def operation(session):
           mesh = MeshModel.find("some-id", session)
           sequences = session.query(PhotoSequence).filter(
               PhotoSequence.mesh_model_id == mesh.id
           ).all()
           return mesh, sequences
       
       # This handles session expiration automatically
       mesh, sequences = safe_session_operation(operation)
       return mesh, sequences
   """)
    
    print("\n7. ASYNC OPERATIONS")
    print("   For async code that might have session issues:")
    print("""
   async def async_photo_processing(mesh_id):
       mesh = MeshModel.find(mesh_id)
       
       # Start async operation
       await start_photo_capture()
       
       # After async operation, refresh before accessing relationships
       mesh.refresh_from_db()
       sequences = mesh._safe_relationship_access('photo_sequences')
       
       return sequences
   """)
    
    print("\n8. DEBUGGING SESSION ISSUES")
    print("   How to debug remaining session problems:")
    print("""
   import logging
   logging.basicConfig(level=logging.DEBUG)
   
   # This will show connection events in the logs
   mesh = MeshModel.find("some-id")
   
   # Check if an instance is attached to a session
   from models.database import is_session_valid
   from sqlalchemy.orm import object_session
   
   session = object_session(mesh)
   if session and is_session_valid(session):
       print("Instance has valid session")
   else:
       print("Instance needs session recovery")
       mesh.refresh_from_db()
   """)

def migration_checklist():
    """Checklist for migrating existing code to use session recovery."""
    
    print("\n=== Migration Checklist ===\n")
    
    checklist = [
        "✓ Database configuration updated with connection pooling",
        "✓ INDBModel enhanced with session recovery methods", 
        "✓ All CRUD operations include retry logic",
        "✓ Safe relationship access methods available",
        "□ Review code for direct relationship access in critical paths",
        "□ Add refresh_from_db() calls after long operations",
        "□ Replace direct relationship access with safe methods where needed",
        "□ Test with actual workload to verify DetachedInstanceError reduction",
        "□ Monitor logs for connection events and remaining issues"
    ]
    
    for item in checklist:
        print(f"   {item}")
    
    print("\n=== Priority Areas to Update ===\n")
    
    priorities = [
        "1. HIGH: Code that accesses relationships after long operations",
        "2. HIGH: Bulk processing operations",
        "3. MEDIUM: Async operations that access relationships", 
        "4. MEDIUM: Error-prone areas where DetachedInstanceError occurred before",
        "5. LOW: Simple CRUD operations (these are already improved automatically)"
    ]
    
    for priority in priorities:
        print(f"   {priority}")

def performance_tips():
    """Performance tips for the session recovery features."""
    
    print("\n=== Performance Tips ===\n")
    
    tips = [
        "• Use eager loading for frequently accessed relationships:",
        "  mesh = session.query(MeshModel).options(joinedload(MeshModel.photo_sequences)).first()",
        "",
        "• Reuse sessions for multiple operations:",
        "  session = Session()",
        "  mesh1 = MeshModel.find(id1, session)",
        "  mesh2 = MeshModel.find(id2, session)",
        "  session.close()",
        "",
        "• Use safe_session_operation for custom queries:",
        "  results = safe_session_operation(lambda s: s.query(Model).filter(...).all())",
        "",
        "• Monitor connection pool usage:",
        "  from models.database import engine",
        "  print(f'Pool size: {engine.pool.size()}')",
        "  print(f'Checked out: {engine.pool.checkedout()}')",
        "",
        "• Adjust pool settings if needed:",
        "  - Increase pool_size for high concurrency",
        "  - Decrease pool_recycle for unstable connections",
        "  - Enable pool logging for debugging"
    ]
    
    for tip in tips:
        print(f"   {tip}")

def main():
    """Run the practical usage guide."""
    
    example_usage_patterns()
    migration_checklist()
    performance_tips()
    
    print("\n🎯 Summary:")
    print("The session recovery features are now active and will help prevent")
    print("DetachedInstanceError exceptions. Most of your existing code will")
    print("work better automatically, but following the patterns above will")
    print("make your application even more robust against connection issues.")

if __name__ == "__main__":
    main()
