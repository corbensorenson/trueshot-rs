#!/usr/bin/env python3
"""
Test the specific Reflex issue where first mesh model finds photo_sequences 
but subsequent ones don't.
"""

import sys
import os
from pathlib import Path

# Set up the path
current_dir = Path(__file__).parent
sys.path.insert(0, str(current_dir))
os.chdir(current_dir)

def test_multiple_mesh_models_reflex_scenario():
    """Test the specific scenario where subsequent mesh models don't find photo_sequences."""
    
    print("=== Testing Multiple Mesh Models (Reflex Scenario) ===\n")
    
    try:
        # Import the necessary components
        from models.database import Session
        from models.indb_model import INDBModel
        from sqlmodel import Field, Relationship
        from sqlalchemy import Column, text
        from sqlalchemy.dialects.postgresql import UUID
        from typing import List, Optional
        import uuid as uuid_pkg
        
        print("✓ Successfully imported all required components")
        
        # Define test models that mimic your actual structure
        class TestMeshModel(INDBModel, table=True):
            __tablename__ = "test_mesh_model_reflex"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Test Mesh")
            description: str = Field(default="")
            
            # Relationship to photo sequences
            photo_sequences: List["TestPhotoSequence"] = Relationship(back_populates="mesh_model")
        
        class TestPhotoSequence(INDBModel, table=True):
            __tablename__ = "test_photo_sequence_reflex"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            description: str = Field(default="Test Sequence")
            rotation_total: int = Field(default=360)
            rotation_step: int = Field(default=10)
            
            # Foreign key to mesh model
            mesh_model_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="test_mesh_model_reflex.id")
            mesh_model: Optional[TestMeshModel] = Relationship(back_populates="photo_sequences")
        
        print("✓ Defined test models mimicking your structure")
        
        # Create the tables
        try:
            from models.database import engine
            from sqlmodel import SQLModel
            SQLModel.metadata.create_all(engine, tables=[TestMeshModel.__table__, TestPhotoSequence.__table__])
            print("✓ Created test tables")
        except Exception as e:
            print(f"⚠ Table creation warning: {e}")
        
        # Test 1: Create multiple mesh models with photo sequences
        print("\n1. Creating multiple mesh models with photo sequences...")
        
        mesh_models = []
        photo_sequences = []
        
        for i in range(3):
            # Create mesh model
            mesh = TestMeshModel(name=f"Reflex Test Mesh {i+1}", description=f"Testing mesh {i+1}")
            mesh.save()
            mesh_models.append(mesh)
            print(f"✓ Created mesh {i+1}: {mesh.id}")
            
            # Create photo sequences for this mesh
            for j in range(2):
                seq = TestPhotoSequence(
                    description=f"Sequence {j+1} for Mesh {i+1}",
                    rotation_total=180,
                    rotation_step=15,
                    mesh_model_id=mesh.id
                )
                seq.save()
                photo_sequences.append(seq)
                print(f"  ✓ Created sequence {j+1} for mesh {i+1}: {seq.id}")
        
        # Test 2: Test normal access (should work for all)
        print("\n2. Testing normal access for all mesh models...")
        for i, mesh in enumerate(mesh_models):
            try:
                fresh_mesh = TestMeshModel.find(mesh.id)
                sequences = fresh_mesh.photo_sequences
                print(f"✓ Mesh {i+1} normal access: {len(sequences)} sequences")
            except Exception as e:
                print(f"✗ Mesh {i+1} normal access failed: {e}")
        
        # Test 3: Simulate Reflex scenario - detached instances
        print("\n3. Simulating Reflex scenario with detached instances...")
        
        detached_meshes = []
        for i, mesh in enumerate(mesh_models):
            # Create detached instance (simulating Reflex state)
            detached_mesh = TestMeshModel()
            detached_mesh.id = mesh.id
            detached_mesh.name = mesh.name
            detached_mesh.description = mesh.description
            detached_meshes.append(detached_mesh)
            print(f"✓ Created detached mesh {i+1}")
        
        # Test 4: Access photo_sequences on each detached mesh (this is where the issue occurs)
        print("\n4. Testing photo_sequences access on detached meshes...")
        
        for i, detached_mesh in enumerate(detached_meshes):
            try:
                # This is the exact pattern from your Reflex app
                sequences = detached_mesh.photo_sequences
                count = len(sequences)
                print(f"✓ Mesh {i+1} detached access: Found {count} photo sequences")
                
                # Test the exact failing line from your Reflex app
                print(f"  Reflex line: Found {len(detached_mesh.photo_sequences)} photo sequences")
                
                if count == 0:
                    print(f"  ⚠ WARNING: Mesh {i+1} found 0 sequences (this might be the issue!)")
                
            except Exception as e:
                print(f"✗ Mesh {i+1} detached access failed: {e}")
                import traceback
                traceback.print_exc()
        
        # Test 5: Try accessing sequences multiple times on the same detached instance
        print("\n5. Testing multiple accesses on same detached instance...")
        
        if detached_meshes:
            test_mesh = detached_meshes[0]
            for attempt in range(3):
                try:
                    sequences = test_mesh.photo_sequences
                    print(f"  Attempt {attempt+1}: Found {len(sequences)} sequences")
                except Exception as e:
                    print(f"  Attempt {attempt+1} failed: {e}")
        
        # Test 6: Force refresh and try again
        print("\n6. Testing with forced refresh...")
        
        for i, detached_mesh in enumerate(detached_meshes):
            try:
                # Force refresh before accessing
                detached_mesh.refresh_from_db()
                sequences = detached_mesh.photo_sequences
                print(f"✓ Mesh {i+1} after refresh: Found {len(sequences)} photo sequences")
            except Exception as e:
                print(f"✗ Mesh {i+1} refresh failed: {e}")
        
        # Cleanup
        print("\n7. Cleaning up...")
        try:
            for seq in photo_sequences:
                TestPhotoSequence.delete(seq.id)
            for mesh in mesh_models:
                TestMeshModel.delete(mesh.id)
            print("✓ Cleanup successful")
        except Exception as e:
            print(f"⚠ Cleanup warning: {e}")
        
        print("\n=== Multiple Mesh Models Test Complete ===")
        return True
        
    except Exception as e:
        print(f"✗ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def main():
    """Run the multiple mesh models test."""
    
    print("Testing Multiple Mesh Models for Reflex Issue...\n")
    
    success = test_multiple_mesh_models_reflex_scenario()
    
    if success:
        print("\n📋 Analysis:")
        print("If you see '0 sequences' for subsequent mesh models, this confirms the issue.")
        print("The enhanced recovery should help, but you might need to:")
        print("1. Force refresh mesh models before accessing relationships")
        print("2. Use a different approach for Reflex state management")
        
        print("\n🔧 Potential Solutions:")
        print("1. Call mesh.refresh_from_db() before accessing photo_sequences")
        print("2. Use safe_getattr() for critical relationship access")
        print("3. Clear and reload mesh models in Reflex state")
        
        return True
    else:
        print("\n❌ Multiple mesh models test failed.")
        return False

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
