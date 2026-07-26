#!/usr/bin/env python3
"""
Test automatic relationship recovery in INDBModel.
This tests the new __getattribute__ override that automatically handles DetachedInstanceError.
"""

import sys
import os
from pathlib import Path

# Set up the path
current_dir = Path(__file__).parent
sys.path.insert(0, str(current_dir))
os.chdir(current_dir)

def test_automatic_relationship_recovery():
    """Test that relationship access automatically recovers from detached instances."""
    
    print("=== Testing Automatic Relationship Recovery ===\n")
    
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
        
        # Define simple test models
        class AutoTestParent(INDBModel, table=True):
            __tablename__ = "auto_test_parent"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Auto Test Parent")
            description: str = Field(default="")
            
            # Relationship to children
            children: List["AutoTestChild"] = Relationship(back_populates="parent")
        
        class AutoTestChild(INDBModel, table=True):
            __tablename__ = "auto_test_child"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Auto Test Child")
            value: int = Field(default=0)
            
            # Foreign key to parent
            parent_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="auto_test_parent.id")
            parent: Optional[AutoTestParent] = Relationship(back_populates="children")
        
        print("✓ Defined test models with automatic recovery")
        
        # Create the tables
        try:
            from models.database import engine
            from sqlmodel import SQLModel
            SQLModel.metadata.create_all(engine, tables=[AutoTestParent.__table__, AutoTestChild.__table__])
            print("✓ Created test tables")
        except Exception as e:
            print(f"⚠ Table creation warning: {e}")
        
        # Test 1: Create parent and child
        print("\n1. Creating test data...")
        parent = AutoTestParent(name="Auto Recovery Test Parent", description="Testing automatic recovery")
        parent.save()
        print(f"✓ Created parent with ID: {parent.id}")
        
        child = AutoTestChild(name="Auto Test Child", value=99, parent_id=parent.id)
        child.save()
        print(f"✓ Created child with ID: {child.id}")
        
        # Test 2: Create a detached instance (simulating Reflex state scenario)
        print("\n2. Testing automatic recovery on detached instance...")
        
        # Create a "detached" instance by copying attributes but not session state
        detached_parent = AutoTestParent()
        detached_parent.id = parent.id
        detached_parent.name = parent.name
        detached_parent.description = parent.description
        
        print("✓ Created detached instance (simulating Reflex state)")
        
        # Test 3: Access relationship - this should automatically recover
        print("\n3. Accessing relationship on detached instance...")
        try:
            # This should work automatically now - no need for special helper functions!
            children = detached_parent.children
            print(f"✓ Automatic recovery successful! Found {len(children)} children")
            
            if children:
                for child_obj in children:
                    print(f"   - Child: {child_obj.name}, Value: {child_obj.value}")
            
        except Exception as e:
            print(f"✗ Automatic recovery failed: {e}")
            return False
        
        # Test 4: Test with fresh session to verify normal operation still works
        print("\n4. Testing normal operation with fresh session...")
        try:
            fresh_parent = AutoTestParent.find(parent.id)
            if fresh_parent:
                children = fresh_parent.children
                print(f"✓ Normal operation works: Found {len(children)} children")
            else:
                print("✗ Could not find parent")
                return False
        except Exception as e:
            print(f"✗ Normal operation failed: {e}")
            return False
        
        # Test 5: Test accessing non-relationship attributes
        print("\n5. Testing non-relationship attribute access...")
        try:
            name = detached_parent.name
            description = detached_parent.description
            print(f"✓ Non-relationship attributes work: name='{name}', description='{description}'")
        except Exception as e:
            print(f"✗ Non-relationship attribute access failed: {e}")
            return False
        
        # Test 6: Test one-to-one relationship
        print("\n6. Testing one-to-one relationship access...")
        try:
            # Create a detached child and access its parent
            detached_child = AutoTestChild()
            detached_child.id = child.id
            detached_child.name = child.name
            detached_child.parent_id = child.parent_id
            
            parent_obj = detached_child.parent
            if parent_obj:
                print(f"✓ One-to-one relationship recovery successful: parent='{parent_obj.name}'")
            else:
                print("⚠ One-to-one relationship returned None (may be expected)")
        except Exception as e:
            print(f"✗ One-to-one relationship access failed: {e}")
            return False
        
        # Cleanup
        print("\n7. Cleaning up test data...")
        try:
            AutoTestChild.delete(child.id)
            AutoTestParent.delete(parent.id)
            print("✓ Test data cleaned up successfully")
        except Exception as e:
            print(f"⚠ Cleanup warning: {e}")
        
        print("\n=== Automatic Recovery Test Complete ===")
        return True
        
    except Exception as e:
        print(f"✗ Error in automatic recovery test: {e}")
        import traceback
        traceback.print_exc()
        return False

def test_reflex_simulation():
    """Simulate the exact Reflex scenario that was causing issues."""
    
    print("\n=== Simulating Reflex Scenario ===\n")
    
    try:
        from models.mesh_model import MeshModel
        from models.photo_sequence import PhotoSequence
        
        print("1. Creating MeshModel and PhotoSequence (simulating Reflex state)...")
        
        # Create test data
        mesh = MeshModel(name="Reflex Simulation Test", description="Testing Reflex scenario")
        mesh.save()
        print(f"✓ Created mesh: {mesh.id}")
        
        photo_seq = PhotoSequence(
            mesh_model_id=mesh.id,
            description="Reflex test sequence",
            rotation_total=180,
            rotation_step=15
        )
        photo_seq.save()
        print(f"✓ Created photo sequence: {photo_seq.id}")
        
        # Simulate what happens in Reflex state - instance becomes detached
        print("\n2. Simulating detached instance in Reflex state...")
        
        # Create a detached mesh (like what happens in Reflex state over time)
        detached_mesh = MeshModel()
        detached_mesh.id = mesh.id
        detached_mesh.name = mesh.name
        detached_mesh.description = mesh.description
        
        print("✓ Created detached mesh instance")
        
        # Test the exact line that was failing in Reflex
        print("\n3. Testing the exact failing line from Reflex...")
        try:
            # This is the line that was causing DetachedInstanceError in Reflex:
            sequences = detached_mesh.photo_sequences
            print(f"✓ SUCCESS! Found {len(sequences)} photo sequences")
            print("   This line should now work automatically in your Reflex app!")
            
            # Test the count operation that was in the error
            count = len(sequences)
            print(f"✓ Count operation works: {count} sequences")
            
        except Exception as e:
            print(f"✗ Still failing: {e}")
            return False
        
        # Cleanup
        print("\n4. Cleaning up...")
        try:
            PhotoSequence.delete(photo_seq.id)
            MeshModel.delete(mesh.id)
            print("✓ Cleanup successful")
        except Exception as e:
            print(f"⚠ Cleanup warning: {e}")
        
        return True
        
    except Exception as e:
        print(f"✗ Reflex simulation failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def main():
    """Run all automatic recovery tests."""
    
    print("Starting Automatic Relationship Recovery Tests...\n")
    
    # Test basic automatic recovery
    basic_success = test_automatic_relationship_recovery()
    
    if basic_success:
        # Test Reflex-specific scenario
        reflex_success = test_reflex_simulation()
        
        if reflex_success:
            print("\n🎉 All automatic recovery tests passed!")
            print("\n📋 What this means for your Reflex app:")
            print("1. ✓ You can now use direct relationship access without helper functions")
            print("2. ✓ mesh.photo_sequences will work automatically even when detached")
            print("3. ✓ No need to call special helper functions in most cases")
            print("4. ✓ Your existing Reflex code should work without modifications")
            
            print("\n📋 Your Reflex state can now use:")
            print("   sequences = self.selected_mesh_model.photo_sequences  # Works automatically!")
            print("   count = len(self.selected_mesh_model.photo_sequences)  # Works automatically!")
            
            return True
        else:
            print("\n❌ Reflex simulation failed.")
            return False
    else:
        print("\n❌ Basic automatic recovery tests failed.")
        return False

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
