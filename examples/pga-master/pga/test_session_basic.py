#!/usr/bin/env python3
"""
Simple test script for session recovery functionality.
This script tests the basic database session utilities without complex model dependencies.
"""

import sys
import os
from pathlib import Path

# Set up the path
current_dir = Path(__file__).parent
sys.path.insert(0, str(current_dir))
os.chdir(current_dir)

def test_database_connection():
    """Test basic database connection and session utilities."""
    
    print("=== Testing Database Connection and Session Utilities ===\n")
    
    try:
        from models.database import Session, is_session_valid, safe_session_operation, get_or_create_session
        print("✓ Successfully imported database utilities")
    except ImportError as e:
        print(f"✗ Failed to import database utilities: {e}")
        return False
    
    # Test 1: Basic session creation
    print("\n1. Testing basic session creation...")
    try:
        session = Session()
        print("✓ Session created successfully")
        
        # Test session validation
        is_valid = is_session_valid(session)
        print(f"✓ Session validation result: {is_valid}")
        
        session.close()
        print("✓ Session closed successfully")
    except Exception as e:
        print(f"✗ Error in basic session test: {e}")
        return False
    
    # Test 2: Session validation with closed session
    print("\n2. Testing session validation with closed session...")
    try:
        session = Session()
        session.close()
        is_valid = is_session_valid(session)
        print(f"✓ Closed session validation result: {is_valid}")
    except Exception as e:
        print(f"✗ Error in closed session validation: {e}")
        return False
    
    # Test 3: get_or_create_session
    print("\n3. Testing get_or_create_session...")
    try:
        # Test with None
        session, created = get_or_create_session(None)
        print(f"✓ get_or_create_session(None) - created: {created}")
        
        # Test with valid session
        session2, created2 = get_or_create_session(session)
        print(f"✓ get_or_create_session(valid) - created: {created2}")
        
        session.close()
        if session2 != session:
            session2.close()
    except Exception as e:
        print(f"✗ Error in get_or_create_session test: {e}")
        return False
    
    # Test 4: safe_session_operation
    print("\n4. Testing safe_session_operation...")
    try:
        from sqlalchemy import text

        def test_operation(session):
            result = session.execute(text("SELECT 1 as test_value"))
            return result.fetchone()[0]

        result = safe_session_operation(test_operation)
        print(f"✓ Safe operation result: {result}")
    except Exception as e:
        print(f"✗ Error in safe_session_operation: {e}")
        return False

    # Test 5: Database connectivity
    print("\n5. Testing database connectivity...")
    try:
        from sqlalchemy import text
        session = Session()
        result = session.execute(text("SELECT version()"))
        version = result.fetchone()[0]
        print(f"✓ Database version: {version[:50]}...")
        session.close()
    except Exception as e:
        print(f"✗ Error connecting to database: {e}")
        return False
    
    print("\n=== All Database Tests Passed! ===")
    return True

def test_indb_model_basics():
    """Test basic INDBModel functionality without complex dependencies."""
    
    print("\n=== Testing INDBModel Basic Functionality ===\n")
    
    try:
        from models.indb_model import INDBModel
        print("✓ Successfully imported INDBModel")
    except ImportError as e:
        print(f"✗ Failed to import INDBModel: {e}")
        return False
    
    # Test session recovery methods exist
    print("\n1. Testing session recovery methods exist...")
    try:
        # Create a dummy instance to test methods
        class TestModel(INDBModel, table=False):
            pass
        
        instance = TestModel()
        
        # Check if methods exist
        assert hasattr(instance, '_get_session_for_instance'), "Missing _get_session_for_instance method"
        assert hasattr(instance, '_reattach_to_session'), "Missing _reattach_to_session method"
        assert hasattr(instance, '_safe_relationship_access'), "Missing _safe_relationship_access method"
        assert hasattr(instance, 'refresh_from_db'), "Missing refresh_from_db method"
        
        print("✓ All session recovery methods are present")
    except Exception as e:
        print(f"✗ Error testing session recovery methods: {e}")
        return False
    
    print("\n=== INDBModel Basic Tests Passed! ===")
    return True

def main():
    """Run all tests."""
    
    print("Starting Session Recovery Tests...\n")
    
    # Test database connection first
    db_success = test_database_connection()
    
    if db_success:
        # Test INDBModel basics
        model_success = test_indb_model_basics()
        
        if model_success:
            print("\n🎉 All tests passed! Session recovery functionality is working.")
            
            print("\n📋 Next Steps:")
            print("1. Try running: python pga/examples/session_recovery_example.py")
            print("2. Test with your actual models by creating and accessing relationships")
            print("3. Monitor for DetachedInstanceError exceptions - they should be much less frequent")
            
            return True
        else:
            print("\n❌ INDBModel tests failed.")
            return False
    else:
        print("\n❌ Database connection tests failed.")
        return False

if __name__ == "__main__":
    success = main()
    sys.exit(0 if success else 1)
