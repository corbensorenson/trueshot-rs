#!/usr/bin/env python3
"""
Example demonstrating the improved session handling in INDBModel.

This example shows how the new session recovery features help handle
Neon database connection timeouts and session expiration.
"""

import sys
import os
from pathlib import Path

# Add the current directory to the path so we can import our models
current_dir = Path(__file__).parent.parent
sys.path.insert(0, str(current_dir))

# Set the working directory to the pga directory
os.chdir(current_dir)

try:
    from models.database import Session, is_session_valid, safe_session_operation
    from models.indb_model import INDBModel
    print("Successfully imported database utilities")
except ImportError as e:
    print(f"Error importing database utilities: {e}")
    sys.exit(1)

# Try to import models, but handle import errors gracefully
try:
    from models.mesh_model import MeshModel
    from models.photo_sequence import PhotoSequence
    MODELS_AVAILABLE = True
    print("Successfully imported MeshModel and PhotoSequence")
except ImportError as e:
    print(f"Warning: Could not import models ({e}). Will demonstrate basic functionality only.")
    MODELS_AVAILABLE = False

import time

def demonstrate_basic_session_utilities():
    """Demonstrate basic session utility functions."""

    print("=== Basic Session Utilities Demonstration ===\n")

    # Test session validation
    print("1. Testing session validation...")
    session = Session()
    print(f"   New session is valid: {is_session_valid(session)}")
    session.close()
    print(f"   Closed session is valid: {is_session_valid(session)}")
    print(f"   None session is valid: {is_session_valid(None)}")

    # Test safe session operation
    print("\n2. Testing safe session operation...")
    def test_operation(session):
        # Simple query that should work
        from sqlalchemy import text
        result = session.execute(text("SELECT 1 as test_value"))
        return result.fetchone()[0]

    try:
        result = safe_session_operation(test_operation)
        print(f"   Safe operation result: {result}")
    except Exception as e:
        print(f"   Error in safe operation: {e}")

    print("\n=== Basic Utilities Complete ===")

def demonstrate_session_recovery():
    """Demonstrate how the improved session handling works."""

    if not MODELS_AVAILABLE:
        print("=== Models not available, skipping model-specific tests ===")
        return

    print("=== Session Recovery Demonstration ===\n")

    # Create a new mesh model
    print("1. Creating a new MeshModel...")
    try:
        mesh = MeshModel(name="Test Mesh for Session Recovery", description="Testing session recovery features")
        mesh.save()
        print(f"   Created MeshModel with ID: {mesh.id}")

        # Create a photo sequence associated with the mesh
        print("\n2. Creating a PhotoSequence...")
        photo_seq = PhotoSequence(
            mesh_model_id=mesh.id,
            description="Test sequence for session recovery",
            rotation_total=180,
            rotation_step=10
        )
        photo_seq.save()
        print(f"   Created PhotoSequence with ID: {photo_seq.id}")

        # Simulate session expiration by waiting and then accessing relationships
        print("\n3. Simulating session expiration...")
        print("   (In real scenarios, this would happen after Neon connection timeout)")

        # Try to access the relationship - this should work with the new session recovery
        print("\n4. Accessing relationships with automatic session recovery...")
        try:
            # This should work even if the original session has expired
            sequences = mesh._safe_relationship_access('photo_sequences')
            if sequences:
                if isinstance(sequences, list):
                    print(f"   Successfully accessed {len(sequences)} photo sequences")
                    for seq in sequences:
                        print(f"   - Sequence: {seq.description}")
                else:
                    print(f"   Successfully accessed photo sequences: {sequences}")
            else:
                print("   No photo sequences found")
        except Exception as e:
            print(f"   Error accessing relationships: {e}")

        # Demonstrate refresh_from_db
        print("\n5. Demonstrating refresh_from_db...")
        try:
            mesh.refresh_from_db()
            print("   Successfully refreshed mesh from database")
        except Exception as e:
            print(f"   Error refreshing from database: {e}")

        # Demonstrate safe save operation
        print("\n6. Demonstrating safe save with potential session recovery...")
        try:
            mesh.description = "Updated description after session recovery test"
            mesh.save()
            print("   Successfully saved updated mesh")
        except Exception as e:
            print(f"   Error saving mesh: {e}")

        # Clean up
        print("\n7. Cleaning up test data...")
        try:
            PhotoSequence.delete(photo_seq.id)
            MeshModel.delete(mesh.id)
            print("   Test data cleaned up successfully")
        except Exception as e:
            print(f"   Error during cleanup: {e}")

    except Exception as e:
        print(f"   Error in demonstration: {e}")
        import traceback
        traceback.print_exc()

    print("\n=== Demonstration Complete ===")

def demonstrate_safe_operations():
    """Demonstrate the safe operation patterns."""

    if not MODELS_AVAILABLE:
        print("=== Models not available, skipping model-specific safe operations ===")
        return

    print("\n=== Safe Operations Demonstration ===\n")

    def custom_query(session):
        """Example custom query operation."""
        return session.query(MeshModel).filter(MeshModel.name.like('%Test%')).all()

    print("1. Using safe_session_operation for custom queries...")
    try:
        results = safe_session_operation(custom_query)
        print(f"   Found {len(results)} mesh models with 'Test' in name")
    except Exception as e:
        print(f"   Error in custom query: {e}")

    print("\n=== Safe Operations Complete ===")

if __name__ == "__main__":
    try:
        demonstrate_basic_session_utilities()
        demonstrate_session_recovery()
        demonstrate_safe_operations()
    except Exception as e:
        print(f"Error in demonstration: {e}")
        import traceback
        traceback.print_exc()
