# from __future__ import annotations

from .indb_model import *
from .job import Job
from .device import Device


# if TYPE_CHECKING:
#     from .job import Job

class Machine(INDBModel, table=True):

    # Must define the id field as a UUID field in each model because attempts to 
    # do so in the INDBModel class result in a pernicious error complaining that id is already defined elsewhere
    id: uuid_pkg.UUID = Field(
        sa_column=Column(UUID(as_uuid=True), 
        server_default=text("gen_random_uuid()"), 
        primary_key=True))
    
    name: str = Field(default="New Model")
    os: str = Field(default="PC")
    cpu: str = Field(default="")
    gpu: str = Field(default="")
    ram: int = Field(default=0)
    # description: str = Field(default="")
    
    description: str = Field(sa_column=Column(TEXT))
    jobs: List[Job] = Relationship(back_populates="machine")
    devices: List[Device] = Relationship(back_populates="machine")

    #process_preferences: JSONB = Field(sa_type=JSONB, nullable=False)
    #priority_mode: str = Field(default="Manual") #model, preference, human
    #status: str = Field(default="Idle")
    #connect_time: datetime = Field(default_factory=datetime.utcnow)
