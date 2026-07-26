from sqlalchemy.orm import sessionmaker
from sqlalchemy import create_engine, event, text
from sqlalchemy.pool import QueuePool
from sqlmodel import SQLModel
import logging

# Configure connection pool for better handling of Neon's connection behavior
# engine = create_engine(os.environ['DATABASE_URL'])
# engine=create_engine("postgresql://jsorenson@localhost:5432/indb")
engine = create_engine(
    "postgresql://ineurodb_owner:OfiKqGWo6Db3@ep-tight-truth-a4eio6o6.us-east-1.aws.neon.tech/indb?sslmode=require",
    poolclass=QueuePool,
    pool_size=5,
    max_overflow=10,
    pool_pre_ping=True,  # Validates connections before use
    pool_recycle=300,    # Recycle connections every 5 minutes
    connect_args={
        "connect_timeout": 10,
        "application_name": "pga_app"
    }
)

Session = sessionmaker(bind=engine)
SQLModel.metadata.clear()

# Add connection event listeners for better debugging
@event.listens_for(engine, "connect")
def receive_connect(dbapi_connection, connection_record):
    logging.debug("Database connection established")

@event.listens_for(engine, "checkout")
def receive_checkout(dbapi_connection, connection_record, connection_proxy):
    logging.debug("Database connection checked out from pool")

@event.listens_for(engine, "checkin")
def receive_checkin(dbapi_connection, connection_record):
    logging.debug("Database connection returned to pool")

def create_db():
    SQLModel.metadata.create_all(engine)

def drop_db():
    SQLModel.metadata.drop_all(engine)

def get_session():
    """Get a new database session with proper error handling."""
    return Session()

def is_session_valid(session):
    """Check if a session is still valid and connected."""
    if session is None:
        return False

    try:
        # Try to execute a simple query to test the connection
        session.execute(text("SELECT 1"))
        return True
    except Exception as e:
        logging.warning(f"Session validation failed: {e}")
        return False

def get_or_create_session(session=None):
    """Get an existing session if valid, otherwise create a new one."""
    if session is not None and is_session_valid(session):
        return session, False  # Return session and whether it was created

    # Close the invalid session if it exists
    if session is not None:
        try:
            session.close()
        except:
            pass

    return Session(), True  # Return new session and that it was created

def safe_session_operation(operation, session=None, close_on_complete=True):
    """
    Execute a database operation with automatic session recovery.

    Args:
        operation: A callable that takes a session as its first argument
        session: Optional existing session to use
        close_on_complete: Whether to close the session after operation

    Returns:
        The result of the operation
    """
    session_created = False

    try:
        if session is None:
            session = Session()
            session_created = True
        elif not is_session_valid(session):
            session.close()
            session = Session()
            session_created = True

        result = operation(session)
        return result

    except Exception as e:
        if session:
            try:
                session.rollback()
            except:
                pass
        raise e
    finally:
        if session and (session_created or close_on_complete):
            try:
                session.close()
            except:
                pass