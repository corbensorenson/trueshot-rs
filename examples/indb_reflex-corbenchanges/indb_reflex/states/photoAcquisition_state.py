import reflex as rx
import asyncio
from typing import Union, Optional, List
import csv
from datetime import datetime, timezone
from sqlmodel import SQLModel, select
from pydantic import Field
import sqlalchemy
from time import sleep
import os
from ..components.navbar_job_dropdown import navbar_job_dropdow_state

from pga import config
from pga.models.database import Session
from pga import INDBModel
from pga import MeshModel
from pga import CameraPreset
from pga import PhotoSequence
from pga import Turntable
from pga import Camera


def get_utc_now() -> datetime:
    return datetime.now(timezone.utc)

class Item(rx.Base):
    """The item class."""

    id: int
    n: int
    orientation: int
    description: str

    

class PAState(rx.State):
    camera_preset_form_name: str = "New Preset"
    model_form_name: str = "New Model"
    photo_sequence_form_name: str = "New Sequence"
    
    mesh_models: List[MeshModel] = []
    selected_mesh_model: Optional[MeshModel] = None
    mesh_model_list: List[str] = ["---"]
    selected_mesh_model_string: str = "---"
    selected_mesh_model_name: str = "---"
    selected_model_notes: str = "---"
    selected_model_description: str = "---"
    
    camera_presets: List[CameraPreset] = []
    selected_camera_preset: Optional[CameraPreset] = None
    camera_presets_list: List[str] = ["---"]
    selected_camera_preset_string: str = "---"
    iso: str = "100"
    aperture: str = "7.1"
    shutter_speed: str = "1/400"
    exposure_mode: str = "Manual"
    camera_preset: str = "General"
    camera_connected: bool = False
    camera_connecting: bool = False
    camera_iso: str = "100"
    camera_aperture: str = "7.1"
    camera_shutter_speed: str = "1/400"
    camera_battery_level: int = 0
    camera_card_present_1: bool = False
    camera_card_usage_1: int = 0
    camera_card_present_2: bool = False
    camera_card_usage_2: int = 0
    camera_card_capacity_1: int = 0
    camera_card_capacity_2: int = 0
    camera_message: str = None
    camera_settings: str = "Connect camera to view settings"
    shutter_speeds: List[str]=[
            "1/1.3", "1/1.6", "1/2", 
            "1/2.5", "1/3", "1/4", "1/5", "1/6", "1/8", "1/10", "1/13", 
            "1/15", "1/20", "1/25", "1/30", "1/40", "1/50", "1/60", "1/80",
            "1/100", "1/125", "1/160", "1/200", "1/250", "1/320", "1/400", 
            "1/500", "1/640", "1/800", "1/1000", "1/1250", "1/1600", "1/2000", 
            "1/2500", "1/3200", "1/4000", "1/5000", "1/6400", "1/8000", "1/10000", 
            "1/13000", "1/16000", "1/20000", "1/26000", "1/32000"
        ]
    
    shutter_speeds_decimal: List[float] = [round(eval(speed), 5) for speed in shutter_speeds]
    
    aperture_values: List[str] = ["1.8", "2", "2.2", "2.5", "2.8", "3.2", "3.5", "4", "4.5",
                         "5", "5.6", "6.3", "7.1", "8", "9", "10", "11", "13", "14", "16"]
    
    iso_values: List[str] = ["64", "80", "100", "125", "160", "200", "250", "320", "400", "500", "640", 
                  "800", "1000", "1250", "1600", "2000", "2500", "3200", "4000", "5000", "6400", 
                  "8000", "10000", "12800", "16000", "20000", "25600"]

    hdr: bool = False
    hdr_exposures: int = 3
    hdr_step_size: str = "1"
    hdr_step_sizes: List[str]=['1/3','2/3', '1','4/3','5/3','2','3']
    hdr_step_sizes_decimal: List[float] = [round(eval(step), 5) for step in hdr_step_sizes]

    checking_auto_shutter_speed: bool = False


    photo_sequences: List[PhotoSequence] = []
    photo_sequences_list: List[str] = []
    selected_photo_sequence: Optional[PhotoSequence] = None
    selected_photo_sequence_string: str = "---"
    selected_photo_sequence_name: str = "---"
    orientation: int  = 1
    orientation_string: str="1"

    rotation_total: int = 360
    rotation_step: int = 5

    turntable_connected: bool = False
    turntable_moving: bool = False
    turntable_message: str = None
    turntable_connecting: bool = False
    show_turntable_controls:bool = False

    photo_sequence_running: bool = False
    current_turntable_step: int = 0
    turntable_steps_remaining: int = 0
    photos_taken: int = 0
    photos_remaining: int = 0
    photos_per_minute: int = 0
    time_elapsed: str = "0:0"
    time_remaining: str = "0:0"
    acquisition_percent_complete: int = 0

    test_pics=[]
    selected_image_settings = []
    started_taking_test_shots: bool = False

    default_test_brightness_values = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]

    focus_stacking: bool = True
    focus_steps: int = 11
    focus_step_width: int = 7 
    lens_end_focus_z: int = 100 #idk what this is need to figure out.
    focus_start_z: int = 25
    focus_end_z: int = 75
    supposed_focus_z: int = 0 #in camera set up it ends with setting the focus to 0.

    thumbnail_location_url = "http://localhost:8000/_upload/thumbnails/"

    arm_connected:bool = False
    arm_connecting:bool = False

    show_thumbnail_section_in_popop:bool = False

    status: str = "Idle"

    selected_mesh_model_id_string:str = ""


    @rx.event
    def handle_url_params(self):
        # Get and process the parameters
        current_url = self.router.page.raw_path
        if 'id=' in current_url:
            model_id = current_url.split('id=')[1]
            print(model_id)
            with rx.session() as session:
                self.selected_mesh_model = session.exec(
                    select(MeshModel).where(MeshModel.id == model_id)
                ).first()
                self.set_mesh_model(self.mesh_model_display_name(self.selected_mesh_model))
                # Redirect to clean URL after processing
                return rx.redirect("/photoAcquisition")
    @rx.event
    def toggle_show_thumbnail_section(self):
        self.show_thumbnail_section_in_popop = not self.show_thumbnail_section_in_popop

    @rx.event
    def setup_state(self):
        self.load_camera_presets()
        self.load_mesh_models()
        self.load_photo_sequences()
        self.handle_url_params()
        

    def toast(self, message: str):
        return rx.toast.success(message, position="top-center")
    
    
    
    # def handle_submit(self, form_data: dict):
    #     self = PA(**form_data)
    #     return rx.toast.success(
    #         "Profile updated successfully", position="top-center"
    #     )
    @rx.event
    def toggle_show_turntable_controls(self):
        self.show_turntable_controls = not self.show_turntable_controls

    def get_number_of_focus_steps(self):
        return int(self.focus_steps)

    def toggle_hdr(self):
        self.hdr = not self.hdr
        self.apply_to_photo_sequence('hdr', self.hdr)

    def toggle_focus_stacking(self):
        self.focus_stacking = not self.focus_stacking
        self.apply_to_photo_sequence('focus_stacking', self.focus_stacking)

    def take_picture(self):
        path=os.path.join('assets/', f'{self.selected_mesh_model.id}.jpg')
        print(f"Taking picture to {path}")
        if(self.camera_connected):
            Camera.camera.capture_preview(path=path)

        return rx.toast.success(
            "Picture taken successfully", position="top-center"
        )
    
    def toggle_image_as_selected(self, setting, value, maxNumberSelected):
        if (setting, value) in self.selected_image_settings:
            self.selected_image_settings.remove((setting, value))
        else:
            if maxNumberSelected == 0 or len(self.selected_image_settings) <= maxNumberSelected:
                self.selected_image_settings.append((setting, value))
            else:
                return rx.toast.error(
                    f"Maximum number of images selected", position="top-center"
                )

    
    def clear_selected_images(self):
        self.selected_image_settings = []
        return rx.toast.success(
            f"Selected images cleared", position="top-center"
        )
    
    def load_focus_stack_test_images(self):
        base_url = "http://localhost:8000/_upload/focus_test/" 
        test_images = []
        self.test_pics = []
        self.started_taking_test_shots = True
        try:
            for i in range(0, int(self.focus_steps) + 1):
                rx.get_upload_url(f"/focus_test/{i}.jpg")
                outfile = rx.get_upload_dir() / f"/focus_test/{i}.jpg"
                self.move_camera_focus(self.focus_step_width)
                Camera.camera.capture_preview(str(outfile))
                test_images.append(base_url + f"{i}.jpg")
        except ValueError as e:
            print(f"Error converting focus_steps to integer: {e}")
        self.started_taking_test_shots = False
        self.test_pics = test_images
    

    def jump_to_focus_start(self):
        self.set_current_focus_z(self.focus_start_z)

    def set_start_focus_to_current(self):
        self.focus_start_z = self.current_focus_z

    def set_end_focus_to_current(self):
        self.focus_end_z = self.current_focus_z

    def jump_to_focus_end(self):
        self.set_current_focus_z(self.focus_end_z)

    def set_current_focus_z(self, distance: int):
        if distance != '':
            temp = distance
            distance = int(distance)
            if(self.camera_connected):
                if int(self.supposed_focus_z) > distance:
                    distance = int(self.supposed_focus_z) - distance
                elif int(self.supposed_focus_z) < distance:
                    distance = distance - int(self.supposed_focus_z)
                Camera.camera.move_focus(distance)
                self.supposed_focus_z = int(temp)
            return rx.toast.success(
                "Camera focus moved", position="top-center"
            )

    def move_camera_focus(self, direction: int):
        if(self.camera_connected):
            self.supposed_focus_z = int(self.supposed_focus_z) + int(direction)
            Camera.camera.move_focus(direction)
        return rx.toast.success(
            "Camera focus moved", position="top-center"
        )
    
    
    def clear_test_pics(self):
        self.test_pics = []
        return rx.toast.success(
            f"test pics cleared", position="top-center"
        )
    @rx.event(background=True)
    async def perform_focus_limit_test(self):
        if(self.camera_connected):
            self.lens_end_focus_z =Camera.camera.perform_focus_limit_test()
        return rx.toast.success(
            "Focus limit test started", position="top-center"
        )

    def decimal_shutter_to_fraction(self, shutter: float):
        index = min(range(len(self.shutter_speeds_decimal)), key=lambda i: abs(self.shutter_speeds_decimal[i] - shutter))
        return self.shutter_speeds[index]
    
    
    @rx.event(background=True)
    async def refresh_camera_info(self):
        if(Camera.camera):
            z=await Camera.camera.get_info()
            
            s="<dl>"
            for k,v in z.items():
                if not isinstance(v, dict):
                    continue
                s+=f"<dt><b>{k}</b></dt>"
                for x,y in v.items():
                    if x=='Camera Date and Time':
                        y = datetime.fromtimestamp(int(y)).strftime('%Y-%m-%d %H:%M:%S')
                    s+=f"<dd>     {x}: {y}</dd>"
            s+="</dl>"
            # print(f"Camera Settings: {s}")
            async with self:
                self.camera_settings=s
                self.camera_battery_level=z['Other PTP Device Properties']['Battery Level']
                self.camera_card_usage_1=z['card_usage_1']
                self.camera_card_usage_2=z['card_usage_2']
                self.camera_card_present_1=z['card_present_1']
                self.camera_card_present_2=z['card_present_2']
                self.camera_card_capacity_1=z['card_capacity_1']
                self.camera_card_capacity_2=z['card_capacity_2']
                self.camera_iso=z['iso']
                self.camera_aperture=z['aperture'].split('/')[1]
                self.camera_shutter_speed = self.decimal_shutter_to_fraction(float(z['shutter_speed'].replace('s', '')))
        # return rx.toast.success(
        #     "Camera settings refreshed", position="top-center"
        # )

    @rx.event(background=True)
    async def poll_camera_info(self):
        while True:
            yield PAState.refresh_camera_info()
            await asyncio.sleep(10)

    @rx.event(background=True)
    async def toggle_camera(self):
        if(Camera.camera):
            async with self:
                self.camera_connecting=True
                self.camera_message="Disconnecting..."

            await Camera.camera.disconnect()
            
            async with self:
                Camera.camera=None
                self.camera_connected=False
                self.camera_message=None
                self.camera_connecting=False
        else:
            #lmao it works but looks funny
            yield navbar_job_dropdow_state.start_new_job("connecting camera....","", -1)
            yield
            async with self:
                self.camera_connecting=True
                self.camera_message="Connecting..."

            await Camera.connect()

            async with self:
                self.camera_message=None
                self.camera_connected=True
                self.status="Camera Connected"
                self.camera_connecting=False
                self.perform_focus_limit_test
            yield navbar_job_dropdow_state.finished_job()
            yield

            yield PAState.poll_camera_info()

        # return rx.toast.success(
        #     f"Camera {"Connected" if self.camera_connected else "Disconnected"}", position="top-center"
        # )
    
    
    @rx.event(background=True)
    async def toggle_turntable(self):
        message='Connected'
        if(Turntable.turntable):
            message='Disconnected'
            async with self:
                self.turntable_message=message
                self.turntable_connecting=True
            await Turntable.turntable.disconnect()

            async with self:
                Turntable.turntable=None
                self.turntable_message=None
                self.turntable_connected=False
                self.turntable_connecting=False
                yield rx.toast.success(
                    f"Turntable {message}", position="top-center"
                )
                yield
        else:
            yield navbar_job_dropdow_state.start_new_job("Connecting Turntable....","", -1)
            yield
            async with self:
                self.turntable_message="Connecting..."
                self.turntable_connecting=True
            # await asyncio.sleep(5) # Turntable.connect()
            await Turntable.connect()
            async with self:
                self.turntable_connected=True
                self.turntable_message=None
                self.turntable_connecting=False
                # Turntable.turntable=Turntable.connect()
                #return rx.toast.success(
                #    f"Turntable {message}", position="top-center"
                #)
            yield navbar_job_dropdow_state.finished_job()
            yield
    
    def set_hdr_exposures(self, hdre: int):
        self.hdr_exposures = int(hdre)
        self.apply_to_photo_sequence('hdr_exposures', int(hdre))
        return self.toast(f"HDR exposures  set to {self.hdr_exposures}")
    
    def set_hdr_step_size(self, hdrstep: str):
        self.hdr_step_size = hdrstep
        self.apply_to_photo_sequence('hdr_step_size', hdrstep)
        return self.toast(f"HDR step size set to {self.hdr_step_size}")

    def set_focus_steps(self, fs: int):
        self.focus_steps = int(fs)
        self.apply_to_photo_sequence('focus_steps', int(fs))
        return self.toast(f"Focus Steps set to {self.focus_steps}")
    
    def set_focus_step_width(self, fsw: int):
        self.focus_step_width = int(fsw)
        self.apply_to_photo_sequence('focus_step_width', int(fsw))
        return self.toast(f"Focus Step Width set to {self.focus_step_width}")
    
    def set_rotation_total(self, rt: int):
        self.rotation_total = int(rt)
        self.apply_to_photo_sequence('rotation_total', int(rt))
        if(Turntable.turntable):
            Turntable.turntable.rotation_total=int(rt)
        return self.toast(f"Rotation Total set to {self.rotation_total} degrees")    
    
    def set_rotation_step(self, rs: int):
        self.rotation_step = int(rs)
        self.apply_to_photo_sequence('rotation_step', int(rs))
        if(Turntable.turntable):
            Turntable.turntable.rotation_step=int(rs)
        return self.toast(f"Rotation Step set to {self.rotation_step} degrees")
    
    @rx.event(background=True)
    async def rotate_cw(self):
        async with self:
            self.turntable_moving=True
        await Turntable.turntable.rotate(int(self.rotation_step))
        async with self:
            self.turntable_moving=False
        return self.toast(f"Turntable Rotation CW")

    @rx.event(background=True)
    async def rotate_ccw(self):
        async with self:
            self.turntable_moving=True
        await Turntable.turntable.rotate(-int(self.rotation_step))
        async with self:
            self.turntable_moving=False
        return self.toast(f"Turntable Rotating CCW")
    
    @rx.event(background=True)
    async def rotate_home(self):
        async with self:
            self.turntable_moving=True
        await Turntable.turntable.rotate_to_origin()
        async with self:
            self.turntable_moving=False
        return self.toast(f"Turntable Rotating to Home Position")
    
    def set_turntable_origin(self):
        Turntable.turntable.reset_origin()
        return self.toast(f"Reset turntable home position")
    
    
    def set_photo_sequence_description(self, ad: str):
        self.acquisition_description = ad
        self.apply_to_photo_sequence('description', ad)
        return self.toast(f"Photo Sequence Description set to {self.acquisition_description}")
    
    def set_orientation(self, orientation: str):
        self.orientation_string = orientation
        self.orientation = int(orientation)
        self.apply_to_photo_sequence('orientation', self.orientation)
        self.update_photo_sequence_list()
        self.copy_photo_sequence_to_state(self.selected_photo_sequence)
        return self.toast(f"Orientation set to {self.orientation}")
    


    


    # @rx.var(cache=True, initial_value=[])
    # def get_items(self) -> list[Item]:
    #     self.load_entries()
    #     return self.items


    # Model Management
    def set_mesh_model(self, model: str):
        print(f"Setting model to {model}")
        if(self.selected_mesh_model_string==model):
            return
        i=self.mesh_model_list.index(model)
        self.copy_mesh_model_to_state(self.mesh_models[i])
        return self.toast(f"Model set to {self.selected_mesh_model_string}")
    
    def mesh_model_display_name(self, m: MeshModel):
        return f"{m.number} - {m.name}".strip()
    
    def copy_mesh_model_to_state(self, m: MeshModel):
        """Copy mesh model data to state with proper error handling."""
        try:
            # Create a fresh session to get the latest data
            with rx.session() as session:
                # Get a fresh copy of the model
                fresh_model = session.get(MeshModel, m.id)
                if not fresh_model:
                    print(f"Model with ID {m.id} not found")
                    return
                
                # Copy data from the fresh model
                self.selected_mesh_model = fresh_model
                self.selected_mesh_model_id_string = str(fresh_model.id)
                self.selected_mesh_model_string = self.mesh_model_display_name(fresh_model)
                self.selected_mesh_model_name = fresh_model.name
                self.selected_model_notes = fresh_model.notes
                self.selected_model_description = fresh_model.description if fresh_model.description else "---"
                
                # Load photo sequences in a separate method with its own session
                self.load_photo_sequences()
                
                print(f"Model set to {self.selected_mesh_model_string}, {self.selected_model_description}")
        except Exception as e:
            print(f"Exception during copy_mesh_model_to_state: {e}")
    
    def update_model_notes(self, notes: str):
        self.selected_model_notes = notes
        self.selected_mesh_model.notes = notes
        print(f"Notes updated to {notes}")
        self.selected_mesh_model.save()

    def update_model_description(self, description: str):
        self.selected_model_description = description
        self.selected_mesh_model.description = description
        print(f"Description updated to {description}")
        self.selected_mesh_model.save()
        return self.toast(f"Model Notes updated")
    
    def new_mesh_model(self):
        n=self.mesh_models[-1].number+1
        self.selected_mesh_model_string = f"{n} - {self.model_form_name}"
        m=MeshModel(number=n, name=self.model_form_name)
        m.save()
        self.mesh_models.append(m)
        self.update_mesh_model_list()
        self.selected_mesh_model=self.mesh_models[n-1]

        print(f"Model {self.selected_mesh_model_string} saved successfully")
        
        return rx.toast.success(
            f"Model {self.selected_mesh_model_string} {n} saved successfully", position="top-center"
        )
    
    def edit_mesh_model(self):
        self.selected_mesh_model.name=self.selected_mesh_model_name
        self.selected_mesh_model.save()
        self.update_mesh_model_list()
        self.copy_mesh_model_to_state(self.selected_mesh_model)
        
        
        return rx.toast.success(
            f"Model {self.selected_mesh_model_string} edited successfully", position="top-center"
        )
    
    def update_mesh_model_list(self):
        ml=[]
        for c in self.mesh_models:
            ml.append(f"{c.number} - {c.name}")
        self.mesh_model_list=ml

    def load_mesh_models(self):
        print("Loading Mesh Models", flush=True)
    
        self.mesh_models=MeshModel.all()
        self.update_mesh_model_list()
        self.copy_mesh_model_to_state(self.mesh_models[0])

    def take_picture_of_model(self):
        pass

    # Camera Preset Management

    # Select a camera preset from the list, which is then applied to the current photo sequence
    def set_camera_preset(self, ps: str):
        if(self.selected_camera_preset_string==ps):
            return
        self.selected_camera_preset_string = ps
        print(f"Cameral Preset set to {ps}")

        if(ps=='Custom'):
            self.selected_camera_preset=None
        else:
            self.selected_camera_preset = next((preset for preset in self.camera_presets if preset.name == ps), None)
            self.copy_camera_preset_to_state(self.selected_camera_preset)
            self.copy_state_to_photo_sequence(self.selected_photo_sequence)
        return self.toast(f"Cameral Preset set to {self.selected_camera_preset_string}")
    
    
    def set_aperture(self, aperture: float):
        self.aperture = aperture
        self.set_camera_preset('Custom')
        self.apply_to_photo_sequence('aperture', aperture)
        return rx.toast.success(
            f"Aperture set to {self.aperture}", position="top-center"
        )
    
    def set_camera_aperture(self, aperture: str):
        Camera.camera.set_aperture(aperture)
        self.camera_aperture=aperture

    def set_iso(self, iso: int):
        self.iso = iso
        self.set_camera_preset('Custom')
        self.apply_to_photo_sequence('iso', iso)
        return self.toast(f"ISO set to {self.iso}")
    
    def set_camera_iso(self, iso: str):
        self.camera_iso=iso
        Camera.camera.set_iso(iso)

    def set_shutter_speed(self, shutter_speed: str):
        self.shutter_speed = shutter_speed
        self.set_camera_preset('Custom')
        self.apply_to_photo_sequence('shutter_speed', shutter_speed)
        return self.toast(f"Shutter Speed set to {self.shutter_speed}")
    
    @rx.event(background=True)
    async def set_to_auto_shutter_speed(self):
        async with self:
            self.checking_auto_shutter_speed=True

        c=Camera.camera
        c.set_exposure_mode('A')
        await asyncio.sleep(2)
        ss=c.get_shutter_speed()
        
        async with self:
            fss=self.decimal_shutter_to_fraction(float(ss.replace('s', '')))

            # async with self:
            self.set_shutter_speed(shutter_speed=fss)
            Camera.camera.set_exposure_mode('M')
            # Camera seems to revert to something else after switching to Manual mode, so reset it to the autoexposure speed
        await asyncio.sleep(2)

        async with self:
            self.set_camera_shutter_speed(fss)
            self.checking_auto_shutter_speed=False
    
    def set_camera_shutter_speed(self, shutter_speed: str):
        self.camera_shutter_speed=shutter_speed
        Camera.camera.set_shutter_speed(shutter_speed)
    
    # Creates a new named preset with the current camera settings
    def new_camera_preset(self):
        self.selected_camera_preset_string = self.camera_preset_form_name
        ps=CameraPreset(name=self.camera_preset_form_name, aperture=self.aperture, iso=self.iso, shutter_speed=self.shutter_speed)
        ps.save()
        self.camera_presets.append(ps)
        self.update_camera_presets_list()
        n=len(self.camera_presets)
        self.selected_camera_preset=self.camera_presets[n-1]
        return rx.toast.success(
            f"Model {self.selected_mesh_model_string} {n} saved successfully", position="top-center"
        )
    
    def save_camera_preset(self):
        self.camera_preset = self.camera_preset_form_name
        print(f"Camera Preset {self.camera_preset} saved successfully")
        return rx.toast.success(
            f"Camera Preset {self.camera_preset} saved successfully", position="top-center"
        )
    
    def update_camera_presets_list(self):
        ml=['Custom']
        for c in self.camera_presets:
            ml.append(c.name)
        self.camera_presets_list=ml
    
    def copy_camera_preset_to_state(self, c: CameraPreset):
        self.selected_camera_preset=c
        self.selected_camera_preset_string=c.name
        self.aperture=c.aperture
        self.iso=c.iso
        self.shutter_speed=c.shutter_speed
    

    def load_camera_presets(self):
        print("Loading Camera Presets", flush=True)
        self.camera_presets=CameraPreset.all()
        self.update_camera_presets_list()
        self.selected_camera_preset=self.camera_presets[0]
        c=self.selected_camera_preset
        self.selected_camera_preset_string=c.name
        print(f"Selected Camera Preset: {self.selected_camera_preset_string}", flush=True)



    def update_photo_sequence_status(self,cs,pt, pr, te, tr, pc):
        self.current_turntable_step=cs
        self.photos_taken=pt
        self.photos_remaining=pr
        self.time_elapsed=te
        self.time_remaining=tr
        self.acquisition_percent_complete=pc

    # Photo Sequence Management
    @rx.event(background=True)
    async def start_photo_sequence(self):
        #print("Starting Photo Sequence")
        seq=self.selected_photo_sequence
        yield navbar_job_dropdow_state.start_new_job(f"starting photo sequence {seq.description}", None, 0)
        yield
        #print(f"Starting Photo Sequence {seq.description}")
        if(seq!=None): # and not seq.photos_completed):
            
            async with self:
                    self.photo_sequence_running=True
                    self.acquisition_percent_complete=0
                    self.status="Running"
                    self.current_turntable_step=0
                    

            st=await seq.prepare_photo_sequence(testing=False, turntable=Turntable.turntable, camera=Camera.camera)
            #print(f"Prepared Photo Sequence: {st}")
            # while(not seq.photos_completed):
            while(not st['completed']):
                st=await seq.next_step(testing=False, st=st)
                yield navbar_job_dropdow_state.update_job(
                    st['percent_complete'],
                    f"Time remaining: {st['time_remaining']}",
                    f"Photos taken: {st['photos_taken']}"
                )
                yield
                # await asyncio.sleep(3) # perform one turntable step with photos
                async with self:
                    self.current_turntable_step=st['step']
                    self.acquisition_percent_complete=st['percent_complete']
                    self.photos_taken=st['photos_taken']
                    self.photos_remaining=st['photos_remaining']
                    self.time_elapsed=st['time_elapsed']
                    self.time_remaining=st['time_remaining']
                    

            async with self:
                self.photo_sequence_running=False
                self.status="Idle"
                seq.photos_completed=True
                seq.photos_taken=st['photos_taken']
                seq.photos_finished=get_utc_now()
                seq.save()

            yield navbar_job_dropdow_state.finished_job()
            yield
            # await seq.acquire_photo_sequence(callback=self.update_photo_sequence_status)

        # print('\a')
        yield self.toast("Photo Sequence Completed")
        yield
    
    def apply_to_photo_sequence(self, vname, value):
        if not self.selected_photo_sequence:
            return
        setattr(self.selected_photo_sequence, vname, value)
        self.selected_photo_sequence.save()
    
    def set_photo_sequence(self, seq: str):
        print(f"Setting Photo Sequence to {seq}")
        if(self.selected_photo_sequence_string==seq):
            return
        i=self.photo_sequences_list.index(seq)
        self.copy_photo_sequence_to_state(self.photo_sequences[i])

        return self.toast(f"Photo Sequence set to {self.selected_photo_sequence_string}")
    

    def copy_photo_sequence_to_state(self, p: PhotoSequence):
        if not p:
            return
        self.selected_photo_sequence=p
        self.selected_photo_sequence_name=p.description
        self.selected_photo_sequence_string=self.photo_sequence_display_name(p)
        self.photo_sequence_form_name = p.description

        self.aperture=p.aperture
        self.iso=p.iso
        self.shutter_speed=p.shutter_speed
        self.hdr=p.hdr
        self.hdr_exposures=p.hdr_exposures
        self.hdr_step_size=p.hdr_step_size
        self.focus_stacking=p.focus_stacking
        self.focus_steps=p.focus_steps
        self.focus_step_width=p.focus_step_width
        self.rotation_total=p.rotation_total
        self.rotation_step=p.rotation_step
        self.orientation=p.orientation
        self.orientation_string=str(p.orientation)

    def copy_state_to_photo_sequence(self, p: PhotoSequence):
        if not p:
            return
        p.aperture=self.aperture
        p.iso=self.iso
        p.shutter_speed=self.shutter_speed
        p.hdr=self.hdr
        p.hdr_exposures=self.hdr_exposures
        p.hdr_step_size=self.hdr_step_size
        p.focus_stacking=self.focus_stacking
        p.focus_steps=self.focus_steps
        p.focus_step_width=self.focus_step_width
        p.rotation_total=self.rotation_total
        p.rotation_step=self.rotation_step
        p.orientation=self.orientation
        p.save()
    
    def new_photo_sequence(self):
        n=len(self.photo_sequences)
        p=PhotoSequence(sequence_number=n+1, description=self.photo_sequence_form_name)
        self.copy_state_to_photo_sequence(p)
        p.mesh_model_id=self.selected_mesh_model.id
        p.save()
        self.photo_sequences.append(p)
        self.update_photo_sequence_list()
        self.selected_photo_sequence=self.photo_sequences[n-1]
        #self.update_photo_sequence_display_name()
        self.selected_photo_sequence_string = self.photo_sequence_display_name(self.selected_photo_sequence)

        print(f"Sequence {self.selected_photo_sequence_string} saved successfully")
        
        return rx.toast.success(
            f"Model {self.selected_photo_sequence_string} saved successfully", position="top-center"
        )

    def edit_photo_sequence(self):
        self.selected_photo_sequence.description = self.photo_sequence_form_name        
        self.selected_photo_sequence.save()
        self.update_photo_sequence_list()
        self.copy_photo_sequence_to_state(self.selected_photo_sequence)

        return rx.toast.success(
            f"Model {self.photo_sequence_form_name} saved successfully", position="top-center"
        )
    
    def photo_sequence_display_name(self, p: PhotoSequence):
        return f"{p.sequence_number} - O({p.orientation}) - {p.description}".strip()

    def update_photo_sequence_list(self):
        if len(self.photo_sequences)==0:
            self.photo_sequences_list=[]
            return
        ml=[]
        for c in self.photo_sequences:
            ml.append(self.photo_sequence_display_name(c))
        self.photo_sequences_list=ml

    def load_photo_sequences(self):
        """Load photo sequences for the selected mesh model with proper session handling."""
        if not self.selected_mesh_model:
            self.photo_sequences = []
            self.update_photo_sequence_list()
            return
        
        try:
            # Create a fresh session for this operation
            with rx.session() as session:
                # Get a fresh copy of the model to avoid stale connections
                model_id = self.selected_mesh_model.id
                fresh_model = session.get(MeshModel, model_id)
                
                if fresh_model:
                    # Explicitly load the photo sequences within this session
                    query = select(PhotoSequence).where(PhotoSequence.mesh_model_id == model_id)
                    self.photo_sequences = session.execute(query).scalars().all()
                    self.update_photo_sequence_list()
                else:
                    self.photo_sequences = []
                    self.update_photo_sequence_list()
            
        except Exception as e:
            print(f"Error loading photo sequences: {e}")
            # Reset to empty list on error
            self.photo_sequences = []
            self.update_photo_sequence_list()
    
    @rx.var
    def thumbnail_upload_location(self) -> str:
        return str(rx.get_upload_dir() / f"thumbnails/{self.selected_mesh_model.id}.jpg")
    
    @rx.var
    def thumbnail_location_string(self) -> str:
        return self.thumbnail_location_url+f"{self.selected_mesh_model.id}.jpg"
    
    @rx.var(cache=False)
    def thumbnail_exists(self) -> bool:
        return os.path.exists(self.thumbnail_upload_location)
    
    @rx.event
    async def save_thumbnail(self, files: list[rx.UploadFile]):
        if not files:
            return
            
        current_file = files[0]  # Get first file since we limited to max_files=1
        upload_data = await current_file.read()
        #outfile = self.get_thumbnail_location()
        # Save the file
        with open(self.thumbnail_upload_location, "wb") as file_object:
            file_object.write(upload_data)

    def capture_thumbnail(self):
        outfile = self.thumbnail_upload_location
        Camera.camera.capture_preview(str(outfile))


