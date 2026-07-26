# from __future__ import annotations
from .indb_model import *
from .photo_sequence import PhotoSequence

# if TYPE_CHECKING:
#     from .photo_sequence import PhotoSequence

class MeshModel(INDBModel, table=True):

    # Must define the id field as a UUID field in each model because attempts to 
    # do so in the INDBModel class result in a pernicious error complaining that id is already defined elsewhere
    id: uuid_pkg.UUID = Field(
        sa_column=Column(UUID(as_uuid=True), 
        server_default=text("gen_random_uuid()"), 
        primary_key=True))
    
    name: str = Field(default="New Model")
    number: int = Field(default=0)
    description: str = Field(default="")
    notes: str = Field(sa_column=Column(TEXT))
    photo_sequences: List[PhotoSequence] = Relationship(back_populates="mesh_model")

    def new_photo_sequence(self,**kwargs):
        photo_sequence = PhotoSequence(**kwargs)
        self.photo_sequences.append(photo_sequence)
        photo_sequence.save()
        return photo_sequence
