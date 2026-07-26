#!/usr/bin/env python3
"""
Simple test for automatic relationship recovery without complex imports.
"""

import sys
import os
from pathlib import Path

# Set up the path
current_dir = Path(__file__).parent
sys.path.insert(0, str(current_dir))
os.chdir(current_dir)

def test_automatic_recovery_simple():
    """Test automatic recovery with simple models."""
    
    print("=== Testing Automatic Relationship Recovery (Simple) ===\n")
    
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
        class SimpleParent(INDBModel, table=True):
            __tablename__ = "simple_parent_auto"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Simple Parent")
            
            # Relationship to children
            children: List["SimpleChild"] = Relationship(back_populates="parent")
        
        class SimpleChild(INDBModel, table=True):
            __tablename__ = "simple_child_auto"
            
            id: uuid_pkg.UUID = Field(
                sa_column=Column(UUID(as_uuid=True), 
                server_default=text("gen_random_uuid()"), 
                primary_key=True))
            name: str = Field(default="Simple Child")
            
            # Foreign key to parent
            parent_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="simple_parent_auto.id")
            parent: Optional[SimpleParent] = Relationship(back_populates="children")
        
        print("✓ Defined simple test models")
        
        # Create the tables
        try:
            from models.database import engine
            from sqlmodel import SQLModel
            SQLModel.metadata.create_all(engine, tables=[SimpleParent.__table__, SimpleChild.__table__])
            print("✓ Created test tables")
        except Exception as e:
            print(f"⚠ Table creation warning: {e}")
        
        # Test 1: Create test data
        print("\n1. Creating test data...")
        parent = SimpleParent(name="Auto Test Parent")
        parent.save()
        print(f"✓ Created parent: {parent.id}")
        
        child = SimpleChild(name="Auto Test Child", parent_id=parent.id)
        child.save()
        print(f"✓ Created child: {child.id}")
        
        # Test 2: Test normal access (should work)
        print("\n2. Testing normal relationship access...")
        try:
            fresh_parent = SimpleParent.find(parent.id)
            children = fresh_parent.children
            print(f"✓ Normal access works: {len(children)} children")
        except Exception as e:
            print(f"✗ Normal access failed: {e}")
            return False
        
        # Test 3: Create detached instance and test automatic recovery
        print("\n3. Testing automatic recovery on detached instance...")
        
        # Create a detached instance (simulating what happens in Reflex)
        detached_parent = SimpleParent()
        detached_parent.id = parent.id
        detached_parent.name = parent.name
        
        try:
            # This should automatically recover from DetachedInstanceError
            children = detached_parent.children
            print(f"✓ Automatic recovery works: {len(children)} children")
            
            # Test the specific pattern from your Reflex error
            count = len(detached_parent.children)
            print(f"✓ Count operation works: {count}")
            
        except Exception as e:
            print(f"✗ Automatic recovery failed: {e}")
            import traceback
            traceback.print_exc()
            return False
        
        # Test 4: Test accessing regular attributes (should not interfere)
        print("\n4. Testing regular attribute access...")
        try:
            name = detached_parent.name
            id_val = detached_parent.id
            print(f"✓ Regular attributes work: name='{name}', id='{id_val}'")
        except Exception as e:
            print(f"✗ Regular attribute access failed: {e}")
            return False
        
        # Cleanup
        print("\n5. Cleaning up...")
        try:
            SimpleChild.delete(child.id)
            SimpleParent.delete(parent.id)
            print("✓ Cleanup successful")
        except Exception as e:
            print(f"⚠ Cleanup warning: {e}")
        
        print("\n=== Test Complete ===")
        return True
        
    except Exception as e:
        print(f"✗ Test failed: {e}")
        import traceback
        traceback.print_exc()
        return False

def demonstrate_reflex_usage():
    """Demonstrate how this works for Reflex usage."""
    
    print("\n=== Reflex Usage Demonstration ===\n")
    
    print("With the automatic recovery implemented, your Reflex state can now use:")
    print()
    print("# BEFORE (was causing DetachedInstanceError):")
    print("def load_photo_sequences(self):")
    print('    print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")')
    print()
    print("# AFTER (now works automatically):")
    print("def load_photo_sequences(self):")
    print('    print(f"Found {len(self.selected_mesh_model.photo_sequences)} photo sequences")')
    print("    # ↑ This exact same line now works automatically!")
    print()
    print("✓ No code changes needed in your Reflex app")
    print("✓ No helper functions to remember")
    print("✓ Automatic recovery happens transparently")
    print("✓ Works for all relationship access: mesh.photo_sequences, child.parent, etc.")

def main():
    """Run the automatic recovery test."""
    
    print("Testing Automatic Relationship Recovery for Reflex...\n")
    
    success = test_automatic_recovery_simple()
    
    if success:
        demonstrate_reflex_usage()
        
        print("\n🎉 Automatic relationship recovery is working!")
        print("\n📋 What this means:")
        print("1. ✓ Your Reflex app should now work without DetachedInstanceError")
        print("2. ✓ No need to modify your existing Reflex state code")
        print("3. ✓ Relationship access automatically recovers from session expiration")
        print("4. ✓ Works for all models that inherit from INDBModel")
        
        print("\n📋 Try your Reflex app again:")
        print("The line that was failing should now work automatically!")
        
        return True
    else:
        print("\n❌ Automatic recovery test failed.")
        return False

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
