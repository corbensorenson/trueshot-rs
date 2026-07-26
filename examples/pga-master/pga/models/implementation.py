# import uuid as uuid_pkg
# from typing import List, TYPE_CHECKING
# from sqlmodel import Field, Relationship
# from sqlalchemy.orm import Mapped, relationship
# from sqlalchemy import text, Column
# from sqlalchemy.dialects.postgresql import UUID, TEXT
# from .indb_model import INDBModel

# class Implementation(INDBModel, table=True):

#     # Must define the id field as a UUID field in each model because attempts to 
#     # do so in the INDBModel class result in a pernicious error complaining that id is already defined elsewhere
#     id: uuid_pkg.UUID = Field(
#         sa_column=Column(UUID(as_uuid=True), 
#         server_default=text("gen_random_uuid()"), 
#         primary_key=True))
#     name: str = Field(default="implementation name")
#     location:str = Field(default="/pga/....")
#     description: str = Field(sa_column=Column(TEXT))
