import importlib
from pga.util.inflection import *
import sys
import asyncio
from pathlib import Path
ppath = Path(__file__).parent.parent 
sys.path.insert(0, str(ppath))
import config
from util.apple_script import AppleScript

import time
from blinker import Signal


class Turntable:
    turntable=None

    @classmethod
    async def connect(cls,implementation_class='Foldio360', rotation_step=5, rotation_total=360, ascript=None):
        cls.turntable = Turntable(implementation_class=implementation_class, rotation_step=rotation_step, rotation_total=rotation_total, ascript=ascript)
        await cls.turntable.initialize()
        return cls.turntable

    async def disconnect(self):
        self.turntable_signal.send(self, data='Disconnecting turntable')
        if(self.implementation):
            self.implementation.disconnect()

    def __init__(self, implementation_class='Foldio360', rotation_step=5, rotation_total=360, ascript=None):
        self.rotation_step = rotation_step
        self.rotation_total = rotation_total
        self.current_rotation = 0
        # implementation_class = globals()[implementation_class]

        turntable_module_file=f"pga_turntable.turntables.{underscore(implementation_class)}"
        turntable_module = importlib.import_module(turntable_module_file)
        turntable_class=getattr(turntable_module,implementation_class)

        self.implementation=turntable_class(rotation_step=rotation_step) if not config.dev_mode else None   
        self.turntable_signal = Signal('turntable_signal')

    async def initialize(self):
        if(self.implementation):
            await self.implementation.initialize()
        else:
            asyncio.sleep(2)

    def reset_origin(self):
        self.current_rotation = 0

    def set_rotation_step(self, rotation_step=None):
        if rotation_step:
            self.rotation_step = rotation_step
    
    def set_rotation_total(self, rotation_total=None):
        if rotation_total:
            self.rotation_total = rotation_total

    async def rotate_to_origin(self):
        await self.rotate_to(0)

    # async def rotate_to(self, degrees, wait=True):
    #     angle=degrees-self.current_rotation
    #     self.rotate(angle, wait=wait)

    async def rotate_to(self, degrees):
        if degrees == self.current_rotation:
            return
        angle = degrees - self.current_rotation
        print(f'Turntable current location is {self.current_rotation} degrees')
        print(f'Turntable rotating to location {degrees} {angle} degrees')
        if angle > 180:
            angle -= 360
        elif angle < -180:
            angle += 360
        print(f'Turntable rotating to location {degrees} with shortcut {angle} degrees')
        await self.rotate(angle, True)

    async def rotate(self,degrees=None, wait=True):
        # print(f'Turntable rotating by {degrees} degrees, debug: {config.dev_mode}')
        if degrees==None:
            degrees = self.rotation_step
        self.current_rotation += degrees
        # print(f'Turntable rotating by {self.rotation_step} degrees, debug: {config.dev_mode}')

        self.turntable_signal.send(self, data=f'Turntable rotating by {degrees} degrees')

        if(self.implementation):
            await self.implementation.rotate(degrees=degrees, wait=wait)  # Forward the rotate method to the implementation object
        else:
            asyncio.sleep(2)

    def rotate_sync(self, degrees=None, wait=True):
        self.loop.create_task(self.rotate(degrees, wait = wait))

    def get_rotation(self):
        return self.current_rotation  # Return the total rotation
    
    def reconnect(self):
        if self.implementation:
            self.implementation.reconnect()





# class Custom:
#     def __init__(self, rotation_step, ascript=None):
#         # Implement the __init__ method for the Custom implementation
#         pass

#     def rotate(self, degrees, wait=True, ascript=None):
#         # Implement the rotate method for the Custom implementation
#         pass
