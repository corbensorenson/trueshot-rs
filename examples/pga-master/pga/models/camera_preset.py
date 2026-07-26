
from .indb_model import *

class CameraPreset(INDBModel, table=True):
    id: uuid_pkg.UUID = Field(
        sa_column=Column(UUID(as_uuid=True), server_default=text("gen_random_uuid()"), primary_key=True))

    name: str = "New Setting..."
    iso: str = "100"
    aperture: str = "7.1"
    shutter_speed: str = "0.001"
    exposure_mode: str = "Manual"
    exposure_compensation: int = 0
    white_balance: str = "Auto"
    