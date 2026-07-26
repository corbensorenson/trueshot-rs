from __future__ import annotations
from .indb_model import *


if TYPE_CHECKING:
    from .machine import Machine

class Device(INDBModel, table=True):

    # Must define the id field as a UUID field in each model because attempts to 
    # do so in the INDBModel class result in a pernicious error complaining that id is already defined elsewhere
    id: uuid_pkg.UUID = Field(
        sa_column=Column(UUID(as_uuid=True), 
        server_default=text("gen_random_uuid()"), 
        primary_key=True))
    
    name: str = Field(default="New Model")
    category: str=Field(default="")
    implementation: str=Field(default="")
    description: str = Field(default="")
    config: str = Field(sa_type=JSONB, nullable=False)
    notes: str = Field(sa_column=Column(TEXT))

    # machine_id: uuid_pkg.UUID = Field(foreign_key="machine.id", nullable=True)
    # machine: "Machine" = Relationship(back_populates="devices")

    machine_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="machine.id")
    # machine: "Machine" = Relationship(back_populates="jobs")
    machine: Machine = Relationship(back_populates="devices")

    