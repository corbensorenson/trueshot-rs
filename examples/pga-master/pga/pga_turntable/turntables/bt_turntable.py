import asyncio
from bleak import BleakScanner, BleakClient
from bleak.exc import BleakDeviceNotFoundError
import json
import os

#combines a bluetooth and turntable class into one.
class BTTurntable:
    def __init__(self, **kwargs):
        self.name = kwargs.get('name', 'Foldio360')
        self.characteristic_uuid = kwargs.get('characteristic_uuid', '6e400002-b5a3-f393-e0a9-e50e24dcca9e')
        self.notify_characteristic_uuid = kwargs.get('notify_characteristic_uuid', '6e400003-b5a3-f393-e0a9-e50e24dcca9e')
        self.address = None
        self.client = None
        self.turning_event = asyncio.Event() #await this event to know when the turntable has finished turning
        # self.loop = asyncio.get_event_loop()
        # self.loop.create_task(self.initialize_client())
        #this should only be turned True when the front end bluetooth select popup is implimented
        #and handle_choose_device is fully implimented
        self.UsingReflex = False
  
    async def initialize(self):        
        async def handle_choose_device(devices):
            if not self.UsingReflex:
                self.choose_device_cli(devices)
            else:
                self.choose_device_reflex(devices)
        try:
            devices = await self.__class__.get_connections_async()
            for device in devices:
                if self.name and device.name == self.name:
                    self.address = device.address
                    break
                elif self.address and device.address == self.address:
                    self.name = device.name
                    break
            #not sure if this for, else thing actually works. cool idea
            # else:
            #     await handle_choose_device(devices)
            self.client = BleakClient(self.address)
            await self.client.connect()
        except BleakDeviceNotFoundError:
            try:
                devices = await self.__class__.get_connections_async()
                await handle_choose_device(devices)
                self.client = BleakClient(self.address)
                await self.client.connect()
            except Exception as e:
                print(f"Failed to connect to {self.name} ({self.address}): {e}")
                self.client = None
        except Exception as e:
            print(f"Failed to connect to {self.name} ({self.address}): {e}")
            self.client = None
        if self.client:
            await self.startNotifications()

    async def startNotifications(self):
        try:
            await self.client.start_notify(self.notify_characteristic_uuid, self.notification_handler)
        except Exception as e:
            print(f"Failed to start notifications on {self.notify_characteristic_uuid}: {e}")


    def notification_handler(self, sender, data):
        if data == b'OK': self.turning_event.set()

    @classmethod
    async def get_connections_async(cls):
        devices = await BleakScanner.discover()
        if not devices:
            print("No devices found.")
            return None
        return devices

    @classmethod
    async def get_connections_sync(cls):
        await cls().get_connections_async()

    #for use in front end to get list of devices
    @classmethod
    def get_BT_connections(cls):
        cls.loop.create_task(cls.get_connections_sync())

    def choose_device_cli(self, devices):
        for i, device in enumerate(devices):
            print(f"{i + 1}: Device: {device.name}, Address: {device.address}")
        choice = int(input("Choose a device by number: ")) - 1
        if choice not in range(len(devices)):
            print("Invalid choice.")
            return None
        device = devices[choice]
        print(f"Selected device: {device.name} ({device.address})")
        self.name = device.name
        self.address = device.address

    def choose_device_reflex(self, devices):
        #plug in front end bluetooth select here, popup with list of devices
        pass
        
    async def reconnect_async(self):
        try:
            if self.client:
                await self.client.disconnect()
            await self.initialize()
        except Exception as e:
            raise e
        
    async def reconnect_sync(self):
        await self.reconnect_async()
    
    def reconnect(self):
        self.loop.create_task(self.reconnect_sync())



if __name__ == "main":
    turntable = BTTurntable()
    turntable.rotate(90)
