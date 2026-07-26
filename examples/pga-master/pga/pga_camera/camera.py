import importlib
import os
import sys
import asyncio
from pathlib import Path

# from .. import config
from blinker import Signal
import time
from pga.util.inflection import *
path_to_config = Path(__file__).parent.parent
sys.path.insert(0, str(path_to_config))
import config

# print(f"dev mode is {config.dev_mode}")



class Camera:
    camera=None
    connected = False

    @classmethod
    async def connect(cls,implementation_class='Nikon'):
        cls.camera = Camera(implementation_class=implementation_class)
        await cls.camera.initialize()
        cls.connected = True
        return cls.camera
    
    async def disconnect(cls):
        cls.camera_signal.send(cls,data='Disconnecting camera')
        cls._delegate('disconnect')
        cls.camera=None



    def __init__(self,**kwargs):
        defaults={
            'implementation_class':'Nikon',
            'aperture':7.1,
            'shutter_speed':0.1,
            'iso':100,
            'white_balance':'Auto',
            'exposure_mode':'Manual',
            'exposure_compensation':0,
            'focus_mode':'Manual', #Starting focus position for focus shifting
            'storage': 'Card',
            'max_focus_z' : 0
        }
        defaults.update(kwargs)

        self.camera_signal=Signal('camera')
        self.camera_signal.send(self,data=f'Initializing camera: type: {defaults["implementation_class"]},  aperture: {defaults["aperture"]}, shutter speed: {defaults["shutter_speed"]}, iso {defaults["iso"]}, white balance: {defaults["white_balance"]}, exposure_mode: {defaults["exposure_mode"]}, exposure_compensation: {defaults["exposure_compensation"]}, storage: {defaults["storage"]}')
        
        camera_module_file=f"pga_camera.cameras.{underscore(defaults['implementation_class'])}"
        camera_module = importlib.import_module(camera_module_file)

        camera_class=getattr(camera_module,defaults['implementation_class'])

        # imp=globals()[defaults['implementation_class']]
        self.implementation=camera_class(defaults['aperture'],defaults['shutter_speed'],defaults['iso']) if not config.dev_mode else None

        # self.set_capture_storage(self.storage)
        # self.set_exposure_mode('M')
        # self.set_aperture(self.aperture)
        # self.set_iso(self.iso)
        # self.set_shutter_speed(self.shutter_speed)

    async def initialize(self):
        if(self.implementation):
            await self.implementation.initialize()
        else:
            time.sleep(3)

    def camera_imp(self):
        return self.implementation
    
    # For special functions that may only be available in the specific camera implementation (creates the missing function to delegate to the implementation)
    def __getattr__(self, name):
        return self._delegate(name)

    def _delegate(self, method_name, *args, **kwargs):
        # self.signal(f'Calling {method_name} with {args} and {kwargs}')
        if self.implementation:
            method = getattr(self.implementation, method_name, None)
            if method:
                # print(f'Calling {method_name} with {args} and {kwargs}')
                return method(*args, **kwargs)
            else:
                raise AttributeError(f"'{self.implementation.__class__.__name__}' object has no attribute '{method_name}'")
        else:
            time.sleep(3)

    def signal(self, s):
        self.camera_signal.send(self, data=s)


    def set_aperture(self, aperture):
        self.aperture = aperture
        # print(f'Setting aperture to {aperture}')
        self.signal(f'Setting aperture to {aperture}')
        self._delegate('set_aperture', aperture)

    def get_shutter_speed(self):
        self.shutter_speed = self._delegate('get_shutter_speed')
        return self.shutter_speed

    def set_shutter_speed(self, shutter_speed):
        self.shutter_speed = shutter_speed
        # print(f'Setting shutter speed to {shutter_speed}')
        self.signal(f'Setting shutter speed to {shutter_speed}')
        self._delegate('set_shutter_speed', shutter_speed)

    def set_hdr_step_size(self, step_size):
        self.hdr_step_size = step_size
        self.signal(f'Setting hdr step size to {step_size}')
        # self._delegate('set_hdr_step_size', step_size)

    def set_exposure_mode(self, value='M'):
        self.exposure_mode=value
        self.signal(f'Setting exposure mode to {value}')
        self._delegate('set_exposure_mode', value)

    def set_iso(self, iso):
        self.iso = iso
        # print(f'Setting iso to {iso}')
        self.signal(f'Setting iso to {iso}')
        self._delegate('set_iso', iso)

    # Not yet implemented in implementation class
    def set_auto_iso(self, value=True):
        self.signal(f'Setting auto iso to {value}')
        self._delegate('set_auto_iso', value)

    def set_capture_storage(self, storage='Card'):
        self.storage = storage
        self.signal(f'Setting capture target to {storage}')
        self._delegate('set_capture_storage', storage)

    def capture_preview(self, path=None):
        self.signal(f'Setting capture target to file')
        return self._delegate('capture_preview',file=path)


    def capture(self, hdr=False, exposures=3, hdr_step='2', file=None):
        self.signal(f'Capturing image hdr={hdr}, exposures={exposures}')
        print(f'Capturing image hdr={hdr}, exposures={exposures}')
        return self._delegate('capture', hdr, exposures, hdr_step, file)
    
    # Switch to aperture priority mode and check the automatic shutter speed
    # def get_auto_shutter_speed(self):
    #     self.set_exposure_mode('A')
    #     # may have to actually take a photo...
    #     time.sleep(1)
    #     # Fetch the shutter speed from the camera
    #     self.shutter_speed = self._delegate('get_shutter_speed')
    #     # self.set_exposure_mode('M')
    #     return self.shutter_speed



    async def get_info(self):
        self.signal(f'Getting camera settings')
        if(self.implementation):
            return self.implementation.get_info()
        return {}

    # Peform sequence of shots with focus shifting, hdr or both
    def perform_shot_sequence(self, focus_shift=True, shots=14, focus_width=4, hdr=False, exposures=3, hdr_step='2', delay=0):
        print(f'Performing shot sequence hdr={hdr}, focus_shift={focus_shift}, shots={shots}, focus_width={focus_width}')
        self.signal(f'Performing focus shifted shot sequence')
        # self._delegate('perform_shot_sequence')
        c=self.implementation
        if c:
            if(focus_shift and c):
                c.set_focus_mode('Manual')
                print(f'about to perform focus shifted shot sequence')
                c.perform_focus_shifted_shot_sequence(shots, focus_width, hdr, exposures, hdr_step, delay)
            elif(hdr and c):
                c.perform_hdr_sequence(exposures)
        else:
            # Dev mode
            time.sleep(3)

        self.signal(f'Shot sequence completed')

        print(f'Shot sequence completed')

    def perform_focus_shift_test(self, shots=14, focus_width=4, savePath=None, delay=0):
        self.signal(f'Performing focus shift test')
        self._delegate('perform_focus_shift_test', shots, focus_width, savePath, delay)
        self.signal(f'Focus shift test completed')

    def perform_focus_limit_test(self):
        self.signal(f'Performing focus limit test')
        self.max_focus_z = self._delegate('perform_focus_limit_test')
        self.signal(f'Focus limit test completed')
        return self.max_focus_z

    def set_Camera_Setting(self, setting, value):
        self.signal(f'Setting {setting} to {value}')
        self._delegate('set_Camera_Setting', setting, value)

    def move_focus(self, distance):
        self.signal(f'Moving focus')
        self._delegate('move_focus', distance)

    def set_focus(self, distance):
        self.signal(f'Moving focus')
        self._delegate('set_focus', distance)

    def get_current_focus(self):
        self.signal("getting current focus")
        return self._delegate('get_current_focus')
    
    def get_max_focus_z(self):
        self.signal("getting max focus")
        return self._delegate('get_max_focus_z')


