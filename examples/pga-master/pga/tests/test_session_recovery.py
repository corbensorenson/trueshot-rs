#!/usr/bin/env python3
"""
Tests for session recovery functionality in INDBModel.
"""

import sys
from pathlib import Path
import unittest
from unittest.mock import patch, MagicMock

# Add the parent directory to the path so we can import our models
ppath = Path(__file__).parent.parent
sys.path.insert(0, str(ppath))

from models.mesh_model import MeshModel
from models.photo_sequence import PhotoSequence
from models.database import Session, is_session_valid, get_or_create_session, safe_session_operation
import sqlalchemy.orm.exc


class TestSessionRecovery(unittest.TestCase):
    """Test cases for session recovery functionality."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.test_mesh = None
        self.test_photo_seq = None
    
    def tearDown(self):
        """Clean up test fixtures."""
        try:
            if self.test_photo_seq:
                PhotoSequence.delete(self.test_photo_seq.id)
            if self.test_mesh:
                MeshModel.delete(self.test_mesh.id)
        except:
            pass
    
    def test_session_validation(self):
        """Test session validation functionality."""
        # Test with valid session
        session = Session()
        self.assertTrue(is_session_valid(session))
        session.close()
        
        # Test with None
        self.assertFalse(is_session_valid(None))
    
    def test_get_or_create_session(self):
        """Test session creation and validation."""
        # Test with None
        session, created = get_or_create_session(None)
        self.assertIsNotNone(session)
        self.assertTrue(created)
        session.close()
        
        # Test with valid session
        valid_session = Session()
        session, created = get_or_create_session(valid_session)
        self.assertEqual(session, valid_session)
        self.assertFalse(created)
        session.close()
    
    def test_safe_session_operation(self):
        """Test safe session operation wrapper."""
        def test_operation(session):
            return session.query(MeshModel).count()
        
        # This should work without throwing exceptions
        result = safe_session_operation(test_operation)
        self.assertIsInstance(result, int)
    
    def test_model_save_with_session_recovery(self):
        """Test that model save works with session recovery."""
        # Create a test mesh
        mesh = MeshModel(name="Test Session Recovery", description="Testing save with session recovery")
        mesh.save()
        self.test_mesh = mesh
        
        # Verify it was saved
        self.assertIsNotNone(mesh.id)
        
        # Update and save again
        mesh.description = "Updated description"
        mesh.save()
        
        # Verify the update
        found_mesh = MeshModel.find(mesh.id)
        self.assertEqual(found_mesh.description, "Updated description")
    
    def test_safe_relationship_access(self):
        """Test safe relationship access."""
        # Create test data
        mesh = MeshModel(name="Test Mesh for Relationships", description="Testing relationship access")
        mesh.save()
        self.test_mesh = mesh
        
        photo_seq = PhotoSequence(
            mesh_model_id=mesh.id,
            description="Test sequence",
            rotation_total=360,
            rotation_step=5
        )
        photo_seq.save()
        self.test_photo_seq = photo_seq
        
        # Test safe relationship access
        sequences = mesh._safe_relationship_access('photo_sequences')
        self.assertIsNotNone(sequences)
        
        # If sequences is a list, check it has our sequence
        if isinstance(sequences, list):
            self.assertGreater(len(sequences), 0)
    
    def test_refresh_from_db(self):
        """Test refresh from database functionality."""
        # Create a test mesh
        mesh = MeshModel(name="Test Refresh", description="Original description")
        mesh.save()
        self.test_mesh = mesh
        
        # Modify the description in memory
        mesh.description = "Modified in memory"
        
        # Refresh from database
        mesh.refresh_from_db()
        
        # Should have original description
        self.assertEqual(mesh.description, "Original description")
    
    def test_find_with_session_recovery(self):
        """Test find method with session recovery."""
        # Create a test mesh
        mesh = MeshModel(name="Test Find", description="Testing find with session recovery")
        mesh.save()
        self.test_mesh = mesh
        
        # Find it back
        found_mesh = MeshModel.find(mesh.id)
        self.assertIsNotNone(found_mesh)
        self.assertEqual(found_mesh.name, "Test Find")
    
    def test_find_by_with_session_recovery(self):
        """Test find_by method with session recovery."""
        # Create a test mesh
        mesh = MeshModel(name="Test FindBy", description="Testing find_by with session recovery")
        mesh.save()
        self.test_mesh = mesh
        
        # Find it using find_by
        results = MeshModel.find_by({"name": "Test FindBy"})
        
        # Results should be a query object, so we need to execute it
        if hasattr(results, 'all'):
            found_meshes = results.all()
            self.assertGreater(len(found_meshes), 0)
            self.assertEqual(found_meshes[0].name, "Test FindBy")
    
    @patch('models.database.Session')
    def test_session_recovery_on_failure(self, mock_session_class):
        """Test that session recovery works when sessions fail."""
        # Create a mock session that fails on first use but works on second
        mock_session = MagicMock()
        mock_session.execute.side_effect = [
            sqlalchemy.orm.exc.DetachedInstanceError("Session expired"),
            MagicMock()  # Second call succeeds
        ]
        mock_session_class.return_value = mock_session
        
        # This should not raise an exception due to session recovery
        try:
            def test_op(session):
                return session.execute("SELECT 1")
            
            safe_session_operation(test_op)
        except sqlalchemy.orm.exc.DetachedInstanceError:
            self.fail("Session recovery should have handled the DetachedInstanceError")


class TestSessionRecoveryIntegration(unittest.TestCase):
    """Integration tests for session recovery with real database operations."""
    
    def setUp(self):
        """Set up test fixtures."""
        self.test_objects = []
    
    def tearDown(self):
        """Clean up test fixtures."""
        for obj in reversed(self.test_objects):
            try:
                if hasattr(obj, 'id') and obj.id:
                    obj.__class__.delete(obj.id)
            except:
                pass
    
    def test_relationship_access_after_session_timeout_simulation(self):
        """Test relationship access after simulating session timeout."""
        # Create test data
        mesh = MeshModel(name="Integration Test Mesh", description="Testing integration")
        mesh.save()
        self.test_objects.append(mesh)
        
        photo_seq = PhotoSequence(
            mesh_model_id=mesh.id,
            description="Integration test sequence",
            rotation_total=180,
            rotation_step=10
        )
        photo_seq.save()
        self.test_objects.append(photo_seq)
        
        # Simulate what happens after a session timeout by creating a new instance
        # that's not attached to any session
        detached_mesh = MeshModel()
        detached_mesh.id = mesh.id
        detached_mesh.name = mesh.name
        detached_mesh.description = mesh.description
        
        # This should work with session recovery
        sequences = detached_mesh._safe_relationship_access('photo_sequences')
        self.assertIsNotNone(sequences)


if __name__ == '__main__':
    # Run the tests
    unittest.main(verbosity=2)
