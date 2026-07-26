from __future__ import annotations
from .indb_model import *
from .mesh_model import MeshModel

if TYPE_CHECKING:
    from .machine import Machine

class Job(INDBModel, table=True):

    # Must define the id field as a UUID field in each model because attempts to 
    # do so in the INDBModel class result in a pernicious error complaining that id is already defined elsewhere
    id: uuid_pkg.UUID = Field(
        sa_column=Column(UUID(as_uuid=True), 
        server_default=text("gen_random_uuid()"), 
        primary_key=True))
    
    name: str = Field(default="New Job")
    processor: str = Field(default="")
    priority: int = Field(default=0)
    status: int = Field(default=0)
    progress: str = Field(default="")
    start_time: datetime = Field(default_factory=datetime.utcnow)
    end_time: datetime = Field(default_factory=datetime.utcnow)
    config: str = Field(sa_type=JSONB, nullable=False)

    # machine_id: uuid_pkg.UUID = Field(foreign_key="machine.id")
    machine_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="machine.id")
    # machine: "Machine" = Relationship(back_populates="jobs")
    machine: Machine = Relationship(back_populates="jobs")

    #mesh_model_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="meshmodel.id")
    # mesh_model: MeshModel = Relationship(sa_relationship=relationship("MeshModel", back_populates='photo_sequences', uselist=False, lazy='immediate'))
    #: MeshModel = Relationship(back_populates="Job")
    #human_interaction: bool = Field(default=False)
