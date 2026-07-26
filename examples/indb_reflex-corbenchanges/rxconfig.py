import reflex as rx
import uuid

# Monkey patch the uuid.UUID class to return a string when jsonified
def uuid_to_json(self):
    return str(self)

uuid.UUID.json = uuid_to_json

# from .. import db_base_config

config = rx.Config(
    app_name="indb_reflex",
    #db_url="sqlite:///indb.db",
    # db_url="postgresql://jeffreysorenson@localhost:5432/indb"
    db_url="postgresql://ineurodb_owner:OfiKqGWo6Db3@ep-tight-truth-a4eio6o6.us-east-1.aws.neon.tech/indb?sslmode=require"
)