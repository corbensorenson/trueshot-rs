# from sqlmodel import SQLModel, Field, Session, select
from sqlmodel import SQLModel, Field, select, Relationship
import uuid as uuid_pkg
import sqlalchemy
from sqlalchemy import event, text, Column
from sqlalchemy.orm import mapper, Mapped, relationship
from sqlalchemy.dialects.postgresql import UUID, JSONB, TEXT
# from sqlalchemy.dialects.postgresql import JSONB

from .database import engine, Session, get_or_create_session, is_session_valid, safe_session_operation
from typing import Optional,Callable, Dict, Any, List, TYPE_CHECKING
from datetime import datetime
import os
import sys
import inspect

# Migrations
# python -m alembic revision --autogenerate -m "Adjusting Schemas"
# python -m alembic upgrade head

bc=None
try:
    import db_base_config as bc
    bc = bc.BASE_MODEL
except:
    print("No db_base_config.py found")
    pass

def get_base_model():
    if bc:
        print(f"Using Base Model : {bc}")
        return bc
    
    # Fallback to environment variable check
    if os.getenv("USE_REFLEX_MODEL") == "1":
        print("Using Reflex as Base MOdel")
        import reflex as rx
        return rx.Model
    
    print("Using SQLModel as base model")
    return SQLModel

BaseModel = get_base_model()

#print(f"Initializing INDBModel - BaseModel: {BaseModel}")





def update_modified_at(mapper, connection, target):
    target.modified_at = datetime.utcnow()


class INDBModel(BaseModel, table=False):
    __abstract__ = True

    created_at: datetime = Field(default_factory=datetime.utcnow)
    modified_at: datetime = Field(default_factory=datetime.utcnow)

    def __getattribute__(self, name):
        """
        Override attribute access to automatically handle DetachedInstanceError
        and empty relationship collections for manually created instances (Reflex scenario).
        """
        # Always use object.__getattribute__ to avoid recursion
        # Skip recovery check for internal attributes to avoid recursion
        if (name.startswith('_') or
            name in {'id', 'created_at', 'modified_at', 'save', 'find', 'delete', 'refresh_from_db',
                    '__class__', '__dict__', '__module__', '__doc__', '__sqlmodel_relationships__',
                    '__table__', '__tablename__', '__mapper__', '__pydantic_core_schema__',
                    '__pydantic_custom_init__', '__pydantic_decorators__', '__pydantic_generic_metadata__',
                    '__pydantic_parent_namespace__', '__pydantic_post_init__', '__pydantic_root_model__',
                    '__pydantic_serializer__', '__pydantic_validator__'}):
            return object.__getattribute__(self, name)

        try:
            # Try normal attribute access first
            result = object.__getattribute__(self, name)

            # Only check for relationship recovery if this looks like a relationship result
            if (hasattr(result, '__len__') and len(result) == 0 and
                hasattr(result, '__class__') and 'InstrumentedList' in str(result.__class__)):
                # This is an empty SQLAlchemy relationship collection
                # Check if we should try to recover it
                try:
                    cls = object.__getattribute__(self, '__class__')
                    id_val = object.__getattribute__(self, 'id')

                    if id_val and hasattr(cls, name):
                        attr = getattr(cls, name)
                        if (hasattr(attr, 'property') and hasattr(attr.property, 'mapper')) or hasattr(attr, 'sa_relationship'):
                            # This is a relationship and we have an ID - try recovery
                            recovered_result = self._handle_detached_relationship_transparent(name)
                            if recovered_result is not None and len(recovered_result) > 0:
                                return recovered_result
                except Exception:
                    pass  # If recovery fails, just return the original result

            return result

        except sqlalchemy.orm.exc.DetachedInstanceError:
            # Handle DetachedInstanceError for relationship attributes
            return self._handle_detached_relationship_transparent(name)
        except Exception:
            # For any other exception, let it propagate normally
            raise

    def _handle_detached_relationship_transparent(self, name):
        """
        Handle detached relationship access with enhanced recovery for Reflex.
        Simplified to avoid recursion issues.
        """
        try:
            cls = object.__getattribute__(self, '__class__')
            id_val = object.__getattribute__(self, 'id')

            if not id_val:
                # No ID, can't recover
                return []

            # Check if this attribute exists on the class and looks like a relationship
            if hasattr(cls, name):
                attr = getattr(cls, name)
                # Simple check for relationship attributes
                if (hasattr(attr, 'property') and hasattr(attr.property, 'mapper')) or hasattr(attr, 'sa_relationship'):
                    # This is a relationship, try direct database query
                    result = self._direct_relationship_query_enhanced(name, id_val)
                    if result is not None:
                        return result

                    # If direct query fails, return appropriate default
                    if hasattr(attr, 'property') and hasattr(attr.property, 'uselist') and attr.property.uselist:
                        return []  # One-to-many relationship
                    else:
                        return None  # One-to-one relationship

            # Not a relationship or can't handle, re-raise
            raise sqlalchemy.orm.exc.DetachedInstanceError(
                f"Instance is detached and attribute '{name}' is not a recoverable relationship"
            )

        except sqlalchemy.orm.exc.DetachedInstanceError:
            # Re-raise DetachedInstanceError as-is
            raise
        except Exception as e:
            # For any other exception during recovery, provide a helpful error
            print(f"Warning: Unexpected error during relationship recovery for '{name}': {e}")
            raise sqlalchemy.orm.exc.DetachedInstanceError(
                f"Instance is detached and recovery failed for attribute '{name}': {e}"
            )

    # def __init__(self, **kwargs):
    #     super().__init__(**kwargs)

    def update_modified_at(mapper, connection, target):
        target.modified_at = datetime.utcnow()

    def safe_getattr(self, name, default=None):
        """
        Safely get an attribute, handling DetachedInstanceError for relationships.
        This is a safer alternative to overriding __getattribute__.

        Usage: sequences = mesh.safe_getattr('photo_sequences', [])
        """
        try:
            return getattr(self, name)
        except sqlalchemy.orm.exc.DetachedInstanceError:
            # Handle detached instance for relationships
            try:
                return self._safe_relationship_access(name)
            except Exception:
                return default
        except Exception:
            return default



    def _get_relationship_default_value(self, relationship_name):
        """
        Get the appropriate default value for a relationship based on its type.
        """
        try:
            # Check if this is a known relationship
            if hasattr(self.__class__, relationship_name):
                attr = getattr(self.__class__, relationship_name)

                # Check if it's a SQLModel Relationship
                if hasattr(attr, 'sa_relationship'):
                    rel = attr.sa_relationship
                    if hasattr(rel, 'property') and hasattr(rel.property, 'uselist'):
                        return [] if rel.property.uselist else None

                # Check if it's a direct SQLAlchemy relationship
                if hasattr(attr, 'property'):
                    if hasattr(attr.property, 'uselist'):
                        return [] if attr.property.uselist else None
                    elif hasattr(attr.property, 'mapper'):
                        # Assume it's a one-to-many if we can't determine
                        return []

                # Check the type annotation to determine if it's a list
                if hasattr(self.__class__, '__annotations__'):
                    annotation = self.__class__.__annotations__.get(relationship_name)
                    if annotation:
                        # Check if it's a List type
                        if hasattr(annotation, '__origin__') and annotation.__origin__ is list:
                            return []
                        elif hasattr(annotation, '__args__') and len(annotation.__args__) > 0:
                            # Check if it's List[SomeType]
                            if str(annotation).startswith('typing.List') or str(annotation).startswith('List'):
                                return []

            # Default fallback - return empty list for safety
            return []

        except Exception:
            # Ultimate fallback
            return []

    def _is_relationship_attribute(self, name):
        """
        Check if an attribute is a relationship.
        """
        try:
            # Use object.__getattribute__ to avoid recursion
            cls = object.__getattribute__(self, '__class__')

            if hasattr(cls, name):
                attr = getattr(cls, name)

                # Check for SQLModel Relationship
                if hasattr(attr, 'sa_relationship'):
                    return True

                # Check for SQLAlchemy relationship
                if hasattr(attr, 'property') and hasattr(attr.property, 'mapper'):
                    return True

                # Check if it's in the relationships registry
                try:
                    relationships = object.__getattribute__(self, '__sqlmodel_relationships__')
                    if name in relationships:
                        return True
                except:
                    pass

            return False
        except Exception:
            return False

    def _get_session_for_instance(self, session=None):
        """
        Get a valid session for this instance, handling session expiration.

        Args:
            session: Optional existing session to validate

        Returns:
            tuple: (session, should_close_session)
        """
        if session is not None:
            if is_session_valid(session):
                return session, False
            else:
                # Session is invalid, close it and create a new one
                try:
                    session.close()
                except:
                    pass

        # Create new session
        new_session = Session()
        return new_session, True

    def _reattach_to_session(self, session):
        """
        Reattach this instance to a session if it's detached.

        Args:
            session: The session to attach to

        Returns:
            The attached instance
        """
        if self not in session and hasattr(self, 'id') and self.id is not None:
            # Instance is detached, merge it back into the session
            return session.merge(self)
        return self

    def _safe_relationship_access(self, relationship_name: str, session=None):
        """
        Safely access a relationship attribute, handling session expiration.

        Args:
            relationship_name: Name of the relationship attribute
            session: Optional session to use

        Returns:
            The relationship value or None if not accessible
        """
        try:
            return getattr(self, relationship_name)
        except sqlalchemy.orm.exc.DetachedInstanceError:
            # Instance is detached, try to reattach and access again
            session, should_close = self._get_session_for_instance(session)
            try:
                reattached = self._reattach_to_session(session)
                result = getattr(reattached, relationship_name)
                return result
            except Exception as e:
                print(f"Failed to access relationship {relationship_name}: {e}")
                return None
            finally:
                if should_close:
                    session.close()

    def _direct_relationship_query_enhanced(self, relationship_name, instance_id):
        """
        Enhanced direct relationship query specifically designed for Reflex usage patterns.
        This bypasses any potential session caching issues.
        """
        try:
            from .database import Session

            # Always use a completely fresh session
            session = Session()

            try:
                # Get the relationship information from the class
                cls = object.__getattribute__(self, '__class__')
                if not hasattr(cls, relationship_name):
                    return None

                attr = getattr(cls, relationship_name)

                # Handle different types of relationships
                if hasattr(attr, 'property') and hasattr(attr.property, 'mapper'):
                    related_class = attr.property.mapper.class_

                    # Check if this is a back reference (one-to-many)
                    if hasattr(attr.property, 'back_populates'):
                        # This is likely a one-to-many relationship
                        # Look for the foreign key field in the related class

                        # Try common foreign key patterns
                        possible_fk_names = [
                            f"{cls.__tablename__}_id",
                            f"{cls.__name__.lower()}_id",
                            f"mesh_model_id",  # Specific to your use case
                        ]

                        for fk_name in possible_fk_names:
                            if hasattr(related_class, fk_name):
                                try:
                                    results = session.query(related_class).filter(
                                        getattr(related_class, fk_name) == instance_id
                                    ).all()
                                    print(f"Debug: Found {len(results)} {relationship_name} using {fk_name}")
                                    return results
                                except Exception as e:
                                    print(f"Debug: Query failed for {fk_name}: {e}")
                                    continue

                        # If no standard foreign key found, try to inspect the relationship
                        if hasattr(attr.property, 'local_columns') and hasattr(attr.property, 'remote_columns'):
                            # Get the actual foreign key columns
                            for local_col, remote_col in zip(attr.property.local_columns, attr.property.remote_columns):
                                results = session.query(related_class).filter(
                                    remote_col == instance_id
                                ).all()
                                print(f"Debug: Found {len(results)} {relationship_name} using relationship columns")
                                return results

                    else:
                        # This might be a many-to-one relationship
                        # Get the foreign key from this instance
                        fk_attr_name = f"{relationship_name}_id"
                        if hasattr(self, fk_attr_name):
                            fk_value = object.__getattribute__(self, fk_attr_name)
                            if fk_value:
                                result = session.query(related_class).filter(
                                    related_class.id == fk_value
                                ).first()
                                print(f"Debug: Found {relationship_name}: {result}")
                                return result

                return None

            finally:
                session.close()

        except Exception as e:
            print(f"Debug: Enhanced direct query failed for {relationship_name}: {e}")
            return None

    def refresh_from_db(self, session=None):
        """
        Refresh this instance from the database, handling session expiration.

        Args:
            session: Optional session to use
        """
        if not hasattr(self, 'id') or self.id is None:
            return self

        session, should_close = self._get_session_for_instance(session)
        try:
            # Get fresh instance from database
            fresh_instance = session.get(self.__class__, self.id)
            if fresh_instance:
                # Update current instance with fresh data
                for key, value in fresh_instance.__dict__.items():
                    if not key.startswith('_'):
                        setattr(self, key, value)
            return self
        finally:
            if should_close:
                session.close()

    def save(self, session=None):
        new_obj = False
        # print(f"Saving {self.__class__.__name__}, session: {session}")

        # Get a valid session, handling expiration
        session, close_session = self._get_session_for_instance(session)

        try:
            if self.id is None:
                new_obj = True
                # print(f"Adding new {self.__class__.__name__} to database")
                session.add(self)
            else:
                if self not in session:
                    # print(f"Object {self.__class__.__name__} is not in session, merging")
                    merged_self = session.merge(self)
                    # Update self with merged instance attributes if needed
                    if hasattr(merged_self, 'id'):
                        self.id = merged_self.id

            session.commit()
            if new_obj:
                session.refresh(self)
        except Exception as e:
            session.rollback()
            # If it's a connection error, try once more with a fresh session
            if "connection" in str(e).lower() or "session" in str(e).lower():
                try:
                    session.close()
                    session = Session()
                    if self.id is None:
                        session.add(self)
                    else:
                        session.merge(self)
                    session.commit()
                    if new_obj:
                        session.refresh(self)
                except:
                    session.rollback()
                    raise
            else:
                raise
        finally:
            if close_session:
                session.close()

    @classmethod
    def all(cls, session=None) -> List["INDBModel"]:
        return cls.find_by({}, session).order_by(cls.created_at).all()

    @classmethod
    def delete(cls, item_id: str, session=None):
        if session is None:
            session = Session()
            close_session = True
        else:
            close_session = False

        try:
            item = cls.find(item_id, session)
            if item:
                session.delete(item)
                session.commit()
        except Exception as e:
            session.rollback()
            # Retry with fresh session if connection error
            if "connection" in str(e).lower() or "session" in str(e).lower():
                try:
                    session.close()
                    session = Session()
                    item = cls.find(item_id, session)
                    if item:
                        session.delete(item)
                        session.commit()
                except:
                    session.rollback()
                    raise
            else:
                raise
        finally:
            if close_session:
                session.close()

    @classmethod
    def copy(cls, item_id: str, session=None):
        if session is None:
            session = Session()
            close_session = True
        else:
            close_session = False

        try:
            item = cls.find(item_id, session)
            if item:
                new_item = cls(**item.dict())
                #need to check if name is in dict
                if hasattr(new_item, "name"):
                    new_item.name = f"{item.name} - Copy"
                new_item.id = None #server should generate new id
                new_item.save(session)
        finally:
            if close_session:
                session.close()
                return new_item
            return None
        
    
    @classmethod
    def find(cls, item_id: int, session=None) -> Optional['INDBModel']:
        def _find_operation(session):
            statement = select(cls).where(cls.id == item_id)
            return session.execute(statement).scalar_one_or_none()

        return safe_session_operation(_find_operation, session, close_on_complete=(session is None))

    # The find_by method returns a SQLAlchemy query object that can be chained with other methods
    # like where(), order_by(), limit(), first(), all(), etc..
    @classmethod
    def find_by(cls, conditions: Dict[str, Any], session=None):
        def _find_by_operation(session):
            return session.query(cls).filter_by(**conditions)

        return safe_session_operation(_find_by_operation, session, close_on_complete=(session is None))

    @classmethod
    def first(cls):
        return cls.find_by({}).order_by(cls.created_at).first()
    
    @classmethod
    def last(cls):
        return cls.find_by({}).order_by(cls.created_at.desc()).first()
    
    # Override for use with rx.Model, dict causes infinite recursion with relationships
    # @classmethod
    # def _dict_recursive(cls, value):
    #     """Recursively serialize the relationship object(s).

    #     Args:
    #         value: The value to serialize.

    #     Returns:
    #         The serialized value.
    #     """
    #     print(f"INDBModel._dict_recursive() called for {value}")

    #     if hasattr(value, "dict"):
    #         print(f"Calling INDBModel._dict() on {value}")
    #         return value.dict()
    #     elif isinstance(value, list):
    #         print(f"Calling INDBModel._dict_recursive() on list {value}")
    #         return [cls._dict_recursive(item) for item in value]
    #     return value
    
    def dict(self, **kwargs):
        """Convert the object to a dictionary with safe relationship handling."""
        try:
            return self.model_dump()
        except Exception:
            # Fallback to manual dict creation if model_dump fails
            base_fields = {name: getattr(self, name) for name in self.__fields__}
            relationships = {}

            # SQLModel relationships do not appear in __fields__, but should be included if present.
            if hasattr(self, '__sqlmodel_relationships__'):
                for name in self.__sqlmodel_relationships__:
                    try:
                        # Use safe relationship access to handle session expiration
                        rel_value = self._safe_relationship_access(name)
                        if rel_value is not None:
                            if hasattr(rel_value, 'dict'):
                                relationships[name] = rel_value.dict()
                            elif isinstance(rel_value, list):
                                relationships[name] = [item.dict() if hasattr(item, 'dict') else item for item in rel_value]
                            else:
                                relationships[name] = rel_value
                    except Exception as e:
                        print(f"Error accessing relationship {name}: {e}")
                        continue

            return {
                **base_fields,
                **relationships,
            }

@event.listens_for(mapper, 'mapper_configured')
def setup_listeners(mapper_instance, class_):
    if issubclass(class_, INDBModel) and class_ is not INDBModel:
        event.listen(class_, 'before_update', update_modified_at)
        event.listen(class_, 'before_insert', update_modified_at)
