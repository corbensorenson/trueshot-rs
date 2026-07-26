#!/usr/bin/env python3
"""
Debug test to understand what's happening with detached instances.
"""

import sys
import os
from pathlib import Path

# Set up the path
current_dir = Path(__file__).parent
sys.path.insert(0, str(current_dir))
os.chdir(current_dir)

def test_debug_detached_behavior():
    """Debug what's happening with detached instances."""
    
    print("=== Debugging Detached Instance Behavior ===\n")
    
    try:
        # Import the necessary components
        from models.database import Session
        from models.indb_model import INDBModel
        from sqlmodel import Field, Relationship
        from sqlalchemy import Column, text
        from sqlalchemy.dialects.postgresql import UUID
        from typing import List, Optional
        import uuid as uuid_pkg
        import sqlalchemy.orm.exc
        
        print("✓ Successfully imported all required components")
        
        # Define test models
        class DebugMeshModel(INDBModel, table=True):
            __tablename__ = "debug_mesh_model"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Debug Mesh")
            
            # Relationship to photo sequences
            photo_sequences: List["DebugPhotoSequence"] = Relationship(back_populates="mesh_model")
        
        class DebugPhotoSequence(INDBModel, table=True):
            __tablename__ = "debug_photo_sequence"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            description: str = Field(default="Debug Sequence")
            
            # Foreign key to mesh model
            mesh_model_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="debug_mesh_model.id")
            mesh_model: Optional[DebugMeshModel] = Relationship(back_populates="photo_sequences")
        
        print("✓ Defined debug models")
        
        # Create the tables
        try:
            from models.database import engine
            from sqlmodel import SQLModel
            SQLModel.metadata.create_all(engine, tables=[DebugMeshModel.__table__, DebugPhotoSequence.__table__])
            print("✓ Created debug tables")
        except Exception as e:
            print(f"⚠ Table creation warning: {e}")
        
        # Create test data
        print("\n1. Creating test data...")
        mesh = DebugMeshModel(name="Debug Test Mesh")
        mesh.save()
        print(f"✓ Created mesh: {mesh.id}")
        
        seq1 = DebugPhotoSequence(description="Debug Sequence 1", mesh_model_id=mesh.id)
        seq1.save()
        seq2 = DebugPhotoSequence(description="Debug Sequence 2", mesh_model_id=mesh.id)
        seq2.save()
        print(f"✓ Created 2 sequences: {seq1.id}, {seq2.id}")
        
        # Test normal access
        print("\n2. Testing normal access...")
        fresh_mesh = DebugMeshModel.find(mesh.id)
        try:
            sequences = fresh_mesh.photo_sequences
            print(f"✓ Normal access: Found {len(sequences)} sequences")
        except Exception as e:
            print(f"✗ Normal access failed: {e}")
        
        # Test what happens with a truly detached instance
        print("\n3. Creating truly detached instance...")
        
        # Method 1: Create instance without session
        detached_mesh1 = DebugMeshModel()
        detached_mesh1.id = mesh.id
        detached_mesh1.name = mesh.name
        print("✓ Created detached mesh (method 1)")
        
        # Method 2: Get instance and close session
        session = Session()
        attached_mesh = session.query(DebugMeshModel).filter(DebugMeshModel.id == mesh.id).first()
        session.close()  # This should make it detached
        print("✓ Created detached mesh (method 2)")
        
        # Test access on both detached instances
        print("\n4. Testing access on detached instances...")
        
        for i, detached_mesh in enumerate([detached_mesh1, attached_mesh], 1):
            print(f"\n  Testing detached mesh {i}:")
            try:
                sequences = detached_mesh.photo_sequences
                print(f"    Result: Found {len(sequences)} sequences")
                print(f"    Type: {type(sequences)}")
                if hasattr(sequences, '__len__'):
                    print(f"    Length: {len(sequences)}")
            except sqlalchemy.orm.exc.DetachedInstanceError as e:
                print(f"    DetachedInstanceError (expected): {e}")
            except Exception as e:
                print(f"    Other error: {e}")
                import traceback
                traceback.print_exc()
        
        # Test with object.__getattribute__ to bypass our override
        print("\n5. Testing with object.__getattribute__ (bypassing our override)...")
        
        try:
            sequences = object.__getattribute__(detached_mesh1, 'photo_sequences')
            print(f"✓ object.__getattribute__: Found {len(sequences)} sequences")
        except sqlalchemy.orm.exc.DetachedInstanceError as e:
            print(f"✓ object.__getattribute__: DetachedInstanceError (expected): {e}")
        except Exception as e:
            print(f"✗ object.__getattribute__: Other error: {e}")
        
        # Test the _safe_relationship_access method directly
        print("\n6. Testing _safe_relationship_access directly...")
        
        try:
            sequences = detached_mesh1._safe_relationship_access('photo_sequences')
            print(f"✓ _safe_relationship_access: Found {len(sequences) if sequences else 0} sequences")
        except Exception as e:
            print(f"✗ _safe_relationship_access failed: {e}")
        
        # Test the enhanced direct query method
        print("\n7. Testing _direct_relationship_query_enhanced directly...")
        
        try:
            sequences = detached_mesh1._direct_relationship_query_enhanced('photo_sequences', mesh.id)
            print(f"✓ _direct_relationship_query_enhanced: Found {len(sequences) if sequences else 0} sequences")
        except Exception as e:
            print(f"✗ _direct_relationship_query_enhanced failed: {e}")
        
        # Cleanup
        print("\n8. Cleaning up...")
        try:
            DebugPhotoSequence.delete(seq1.id)
            DebugPhotoSequence.delete(seq2.id)
            DebugMeshModel.delete(mesh.id)
            print("✓ Cleanup successful")
        except Exception as e:
            print(f"⚠ Cleanup warning: {e}")
        
        print("\n=== Debug Test Complete ===")
        return True
        
    except Exception as e:
        print(f"✗ Debug test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def main():
    """Run the debug test."""
    
    print("Debugging Detached Instance Behavior...\n")
    
    success = test_debug_detached_behavior()
    
    if success:
        print("\n📋 Analysis:")
        print("This test helps understand what's happening with detached instances.")
        print("Look for:")
        print("1. Whether DetachedInstanceError is actually being raised")
        print("2. Whether our recovery methods are working")
        print("3. What type of result is being returned")
        
        return True
    else:
        print("\n❌ Debug test failed.")
        return False

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
