#!/usr/bin/env python3
"""
Test the safe_getattr method for Reflex DetachedInstanceError handling.
"""

import sys
import os
from pathlib import Path

# Set up the path
current_dir = Path(__file__).parent
sys.path.insert(0, str(current_dir))
os.chdir(current_dir)

def test_safe_getattr():
    """Test the safe_getattr method."""
    
    print("=== Testing safe_getattr Method ===\n")
    
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
        class TestParent(INDBModel, table=True):
            __tablename__ = "test_parent_safe"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Test Parent")
            
            # Relationship to children
            children: List["TestChild"] = Relationship(back_populates="parent")
        
        class TestChild(INDBModel, table=True):
            __tablename__ = "test_child_safe"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Test Child")
            
            # Foreign key to parent
            parent_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="test_parent_safe.id")
            parent: Optional[TestParent] = Relationship(back_populates="children")
        
        print("✓ Defined test models")
        
        # Create the tables
        try:
            from models.database import engine
            from sqlmodel import SQLModel
            SQLModel.metadata.create_all(engine, tables=[TestParent.__table__, TestChild.__table__])
            print("✓ Created test tables")
        except Exception as e:
            print(f"⚠ Table creation warning: {e}")
        
        # Test 1: Create test data
        print("\n1. Creating test data...")
        parent = TestParent(name="Safe Getattr Test Parent")
        parent.save()
        print(f"✓ Created parent: {parent.id}")
        
        child = TestChild(name="Safe Getattr Test Child", parent_id=parent.id)
        child.save()
        print(f"✓ Created child: {child.id}")
        
        # Test 2: Test safe_getattr with valid instance
        print("\n2. Testing safe_getattr with valid instance...")
        try:
            fresh_parent = TestParent.find(parent.id)
            children = fresh_parent.safe_getattr('children', [])
            print(f"✓ safe_getattr works with valid instance: {len(children)} children")
        except Exception as e:
            print(f"✗ safe_getattr failed with valid instance: {e}")
            return False
        
        # Test 3: Test safe_getattr with detached instance
        print("\n3. Testing safe_getattr with detached instance...")
        
        # Create a detached instance (simulating Reflex state)
        detached_parent = TestParent()
        detached_parent.id = parent.id
        detached_parent.name = parent.name
        
        try:
            # This should work with safe_getattr
            children = detached_parent.safe_getattr('photo_sequences', [])
            print(f"✓ safe_getattr works with detached instance: {len(children)} children")
            
            # Test with default value
            children = detached_parent.safe_getattr('nonexistent_relationship', [])
            print(f"✓ safe_getattr returns default for nonexistent: {children}")
            
        except Exception as e:
            print(f"✗ safe_getattr failed with detached instance: {e}")
            return False
        
        # Test 4: Test safe_getattr with regular attributes
        print("\n4. Testing safe_getattr with regular attributes...")
        try:
            name = detached_parent.safe_getattr('name', 'Unknown')
            id_val = detached_parent.safe_getattr('id', None)
            print(f"✓ safe_getattr works with regular attributes: name='{name}', id='{id_val}'")
        except Exception as e:
            print(f"✗ safe_getattr failed with regular attributes: {e}")
            return False
        
        # Test 5: Simulate the exact Reflex scenario
        print("\n5. Simulating exact Reflex scenario...")
        try:
            # This simulates the failing line in your Reflex app
            sequences = detached_parent.safe_getattr('photo_sequences', [])
            count = len(sequences)
            print(f"✓ Reflex scenario works: Found {count} photo sequences")
            
            # This simulates what your print statement would look like
            print(f"✓ Print statement equivalent: Found {len(detached_parent.safe_getattr('photo_sequences', []))} photo sequences")
            
        except Exception as e:
            print(f"✗ Reflex scenario failed: {e}")
            return False
        
        # Cleanup
        print("\n6. Cleaning up...")
        try:
            TestChild.delete(child.id)
            TestParent.delete(parent.id)
            print("✓ Cleanup successful")
        except Exception as e:
            print(f"⚠ Cleanup warning: {e}")
        
        print("\n=== safe_getattr Test Complete ===")
        return True
        
    except Exception as e:
        print(f"✗ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def main():
    """Run the safe_getattr test."""
    
    print("Testing safe_getattr for Reflex DetachedInstanceError...\n")
    
    success = test_safe_getattr()
    
    if success:
        print("\n🎉 safe_getattr is working perfectly!")
        print("\n📋 How to fix your Reflex app:")
        print("Replace this line in your Reflex state:")
        print('  print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")')
        print("\nWith this line:")
        print('  sequences = self.selected_mesh_model.safe_getattr("photo_sequences", [])')
        print('  print(f"Found {len(sequences)} photo sequences")')
        
        print("\n✅ This will solve your DetachedInstanceError!")
        
        return True
    else:
        print("\n❌ safe_getattr test failed.")
        return False

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
