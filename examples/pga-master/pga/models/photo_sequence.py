from __future__ import annotations # This is needed for the type hinting of the class itself, avoiding circular references
from datetime import datetime, timedelta
import asyncio
from time import sleep
from pydantic import PrivateAttr


import sys
from pathlib import Path

ppath = Path(__file__).parent.parent 
sys.path.insert(0, str(ppath))
import config
sys.path.insert(0, str(ppath / "pga_camera"))
from pga_camera.camera import Camera
sys.path.insert(0, str(ppath  / "pga_turntable"))
from pga_turntable.turntable import Turntable

from .camera_preset import CameraPreset
from .database import engine, Session
from .indb_model import *


# Avoid the circular import problem by using TYPE_CHECKING
if TYPE_CHECKING:
    from .mesh_model import MeshModel
    from .camera_preset import CameraPreset

# from .. import config

class PhotoSequence(INDBModel, table=True):
    id: uuid_pkg.UUID = Field(
        sa_column=Column(UUID(as_uuid=True), server_default=text("gen_random_uuid()"), primary_key=True))


    camera_name: str = Field(default="Nikon")

    aperture: str = Field(default="7.1")
    iso: str = Field(default="100")
    shutter_speed: str = Field(default="1/100")
    exposure_mode: str = Field(default="M")
    
    # camera_preset_id: Optional[int] = Field(default=None, foreign_key="camerapreset.id")
    camera_preset_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="camerapreset.id")
    camera_preset: CameraPreset = Relationship(sa_relationship=relationship("CameraPreset", uselist=False, lazy='immediate'))

    hdr: bool = Field(default=False)
    hdr_exposures: int = Field(default=3)
    hdr_starting_shutter_speed: str = Field(default="1/100")
    hdr_step_size: str = Field(default="1")

    focus_stacking: bool = Field(default=True)
    focus_steps: int = Field(default=11)
    focus_step_width: int = Field(default=7)

    mesh_model_id: Optional[uuid_pkg.UUID] = Field(default=None, foreign_key="meshmodel.id")
    # mesh_model: MeshModel = Relationship(sa_relationship=relationship("MeshModel", back_populates='photo_sequences', uselist=False, lazy='immediate'))
    mesh_model: MeshModel = Relationship(back_populates="photo_sequences")
    # mesh_model_name: str = Field(default="")

    rotation_total: int = Field(default=360)
    rotation_step: int = Field(default=5)

    sequence_number: int = Field(default=1)
    orientation: int = Field(default=1)
    description: str  = Field(default="")

    current_turntable_step: int = Field(default=0)
    turntable_steps_remaining: int = Field(default=0)
    photos_taken: int = Field(default=0)
    photos_remaining: int = Field(default=0)
    photos_per_minutes: float = Field(default=0.0)
    time_elapsed: int = Field(default=0)
    time_remaining: int = Field(default=0)
    percennt_complete: float = Field(default=0.0)

    status: str = Field(default="Idle")

    photos_start: Optional[datetime] = Field(default=None)
    photos_finish: Optional[datetime] = Field(default=None)
    photos_completed: bool = Field(default=False)
    transfer_start: Optional[datetime] = Field(default=None)
    transfer_finish: Optional[datetime] = Field(default=None)
    transfer_completed: bool = Field(default=False)
    focus_start: Optional[datetime] = Field(default=None)
    focus_finish: Optional[datetime] = Field(default=None)
    focus_completed: bool = Field(default=False)
    hdr_start: Optional[datetime] = Field(default=None)
    hdr_finish: Optional[datetime] = Field(default=None)
    hdr_completed: bool = Field(default=False)
    background_start: Optional[datetime] = Field(default=None)
    background_finish: Optional[datetime] = Field(default=None)
    background_removed: bool = Field(default=False)

    _callback: Optional[Callable] = PrivateAttr(default=None)


    def __init__(self, **data):
        super().__init__(**data)
        # self._callback=data.get("callback", None)
        if(not self.photos_remaining):
            self.photos_remaining = round((self.rotation_total / self.rotation_step) * self.focus_steps)
            if self.hdr:
                self.photos_remaining *= self.hdr_exposures

    class Config:
        arbitrary_types_allowed = True

    # Use for command line synchronous acquisition
    def acquire_photo_sequence(self, turntable=None, camera=None, callback: Optional[Callable] = None):
        print(f"Development mode: {config.dev_mode}")
        self._callback = callback

        self.photos_start = datetime.now()
        if(not turntable):
            turntable=Turntable.turntable if Turntable.turntable else Turntable.connect(implementation_class='Foldio360', rotation_step=5, rotation_total=360)
        turntable.set_rotation_step(self.rotation_step)
        turntable.set_rotation_total(self.rotation_total)   
        
        if(not camera):
            camera=Camera.camera if Camera.camera else Camera.connect(implementation_class=self.camera_name)
        print("Settings are camera: ", self.camera_name, " aperture: ", self.aperture, " shutter_speed: ", self.shutter_speed, " iso: ", self.iso)
        camera.set_aperture(self.aperture)
        camera.set_shutter_speed(self.shutter_speed)
        camera.set_hdr_step_size(self.hdr_step_size)
        camera.set_iso(self.iso)
        
        images_per_step=self.focus_steps * (1 if not self.hdr else self.hdr_exposures)

        while turntable.get_rotation() < self.rotation_total:
            camera.perform_shot_sequence(self.focus_stacking, self.focus_steps, self.focus_step_width, self.hdr, self.hdr_exposures)

            self.current_turntable_step += 1
            self.photos_taken += images_per_step
            self.photos_remaining -= images_per_step

            turntable.rotate(self.rotation_step, wait=True)

            if self._callback:
                self._callback(self.current_turntable_step, self.photos_taken, self.photos_remaining, self.time_elapsed, self.time_remaining, self.percennt_complete)
       
        self.photos_finish = datetime.now()
        self.photos_completed = True
        self.save()

    # Use for asynchronous acquisition along with next_step, passing the state each time since we cannot alter the object state in the async loop
    async def prepare_photo_sequence(self, turntable=None, camera=None, testing=False, callback: Optional[Callable] = None):
        print(f"Development mode: {config.dev_mode}")
        print(f"Turntable: {turntable}")
        print(f"Turntble.turntable: {Turntable.turntable}")
        print(f"Camera: {Camera.camera}")

        #For some reason the class variables are not available here, perhaps due the class object not being global
        if turntable and not Turntable.turntable:
            Turntable.turntable = turntable

        if camera and not Camera.camera:
            Camera.camera = camera

        if not testing:
            if not turntable:
                turntable=Turntable.turntable if Turntable.turntable else await Turntable.connect(implementation_class='Foldio360', rotation_step=5, rotation_total=360)
            turntable.set_rotation_step(self.rotation_step)
            turntable.set_rotation_total(self.rotation_total)   
            
            if not camera:
                camera=Camera.camera if Camera.camera else await Camera.connect(implementation_class=self.camera_name)
            print("Settings are camera: ", self.camera_name, " aperture: ", self.aperture, " shutter_speed: ", self.shutter_speed, " iso: ", self.iso)
            camera.set_aperture(self.aperture)
            camera.set_shutter_speed(self.shutter_speed)
            camera.set_hdr_step_size(self.hdr_step_size)
            camera.set_iso(self.iso)
            camera.set_capture_storage('Card')
        else:
            await asyncio.sleep(2)

        return {
                    'step': 0, 
                    'photos_taken': 0, 
                    'photos_remaining': self.photos_remaining, 
                    'time_elapsed': "00:00", 
                    'time_remaining': "00:00", 
                    'percent_complete': "0",
                    'start_time': datetime.now(),
                    'completed': False
                }


    async def next_step(self, st={}, testing=False):
        print (f"Next step: {st} of {round(self.rotation_total / self.rotation_step)} total steps")
        print(f"Turntable rotation:: {Turntable.turntable.get_rotation()}, total_rotation: {self.rotation_total}")
        if((testing or Turntable.turntable.get_rotation() < self.rotation_total) and (st['step'] < round(self.rotation_total / self.rotation_step))):
            images_per_step=self.focus_steps * (1 if not self.hdr else self.hdr_exposures)
            print(f"Performing shot sequence: focus_stacking={self.focus_stacking}, focus_steps={self.focus_steps}, focus_step_width={self.focus_step_width}, hdr={self.hdr}, hdr_exposures={self.hdr_exposures}")
            Camera.camera.perform_shot_sequence(self.focus_stacking, self.focus_steps, self.focus_step_width, self.hdr, self.hdr_exposures, self.hdr_step_size)
            if testing:
                sleep(3)
            await Turntable.turntable.rotate(self.rotation_step, wait=True)

            t=datetime.now()
            return {
                    'step': st['step']+1, 
                    'photos_taken': st['photos_taken']+images_per_step, 
                    'photos_remaining': st['photos_remaining']-images_per_step, 
                    'time_elapsed':  str(t - st['start_time']).split('.')[0], 
                    'time_remaining': str(((t - st['start_time']) / (st['step']+1)) * ((self.rotation_total / self.rotation_step)-st['step']-1)).split('.')[0], 
                    'percent_complete': str(round((st['step']+1) / (self.rotation_total / self.rotation_step) * 100)),
                    'start_time': st['start_time'],
                    'completed': False
                    }
        else:
            t=datetime.now()
            st['completed'] = True
            st['photos_finish'] = datetime.now()
            st['percent_complete'] = 100
            st['time_elapsed']= str(t - st['start_time']).split('.')[0], 
            st['time_remaining'] = "00:00"
            return st