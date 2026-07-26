import asyncio
import time
from bleak import BleakScanner, BleakClient
from bleak.exc import BleakDeviceNotFoundError


# send command cancel(0) to stop the turntable
class TurntableBluetooth:
    def __init__(self, name='Foldio360', rotation_step=5, address='69420', debug=False):
        self.name = name
        self.rotation_step = rotation_step
        self.address = address
        self.currentLocation = 0
        self.debug = debug
        self.characteristic_uuid = "6e400002-b5a3-f393-e0a9-e50e24dcca9e"
        self.notify_characteristic_uuid = "6e400003-b5a3-f393-e0a9-e50e24dcca9e"
        self.device = None
        self.client = None
        self.brightness = 0
        self.turning_event = asyncio.Event()
  
    async def initialize_client(self):
        try:
            self.client = BleakClient(self.address)
            await self.client.connect()
            if self.debug:
                print(f"Connected to {self.address}")
                await self.discover_services()
        except BleakDeviceNotFoundError:
            try:
                devices = await self.scan_bluetooth_devices()
                device = await self.choose_device(devices)
                if device:
                    self.address = device.address
                    self.client = BleakClient(self.address)
                    await self.client.connect()
                    if self.debug:
                        print(f"Connected to {self.address}")
                        await self.discover_services()
            except Exception as e:
                print(f"Failed to connect to {self.name} ({self.address}): {e}")
                self.client = None
        except Exception as e:
            print(f"Failed to connect to {self.name} ({self.address}): {e}")
            self.client = None
        try:
            await self.client.start_notify(self.notify_characteristic_uuid, self.notification_handler)
            if self.debug:
                print(f"Started notifications on {self.notify_characteristic_uuid}")
        except Exception as e:
            print(f"Failed to start notifications on {self.notify_characteristic_uuid}: {e}")

    def notification_handler(self, sender, data):
        # Process the notification data
        if self.debug:
            print(f"Notification from {sender}: {data}")
        # Update the state based on the notification data
        if data == b'OK':  # Assuming 'OK' is the notification data when the table stops
            self.turning_event.set()

    async def rotate_to(self, location):
        if location == self.currentLocation:
            return
        angle = location - self.currentLocation
        if angle > 180:
            angle -= 360
        elif angle < -180:
            angle += 360
        await self.rotate(angle, True)
        

    async def rotate(self, degrees=None, wait=True):
        if degrees is None:
            degrees = self.rotation_step
        asyncio.create_task(self.turn_table(degrees))
        # self.turn_table(degrees)

    async def turn_table(self, angle,speed=3):
        if self.client is None or not self.client.is_connected:
            print(f"Client is not connected to {self.address}")
            return
        direction = "CW" if angle > 0 else "CCW"
        command = f"rotate({direction},{angle},{speed},1)".encode()
        self.wait_for_turning_to_stop()
        self.turning_event.clear()
        await self.client.write_gatt_char(self.characteristic_uuid, command, response=True)
        if self.debug:
            print(f"Command sent: {command}")
        self.currentLocation += angle
        self.currentLocation %= 360

    # async def turn_table(self, angle, direction="CCW", speed=3):
    #     if self.client is None or not self.client.is_connected:
    #         print(f"Client is not connected to {self.address}")
    #         return
    #     command = f"rotate({direction},{angle},{speed},1)".encode()
    #     self.wait_for_turning_to_stop()
    #     self.turning_event.clear()
    #     await self.client.write_gatt_char(self.characteristic_uuid, command, response=True)
    #     if self.debug:
    #         print(f"Command sent: {command}")
    #     if direction == "CCW":
    #         self.currentLocation += angle
    #     else:
    #         self.currentLocation -= angle
    #     self.currentLocation %= 360

    # async def turn_table_cw(self, angle, speed=3):
    #     asyncio.create_task(self.turn_table(angle, "CW", speed))

    # async def turn_table_ccw(self, angle, speed=3):
    #     asyncio.create_task(self.turn_table(angle, "CCW", speed))
    
    async def wait_for_turning_to_stop(self):
        await self.turning_event.wait()

    async def set_brightness(self, brightness):
        command = f"set_bright({brightness})".encode()
        if self.client.is_connected:
            if self.debug:
                print(f"Connected to {self.address}")
            await self.client.write_gatt_char(self.characteristic_uuid, command)
            self.brightness = brightness
            if self.debug:
                print(f"Command sent to {self.characteristic_uuid}")
        else:
            print(f"Failed to connect to {self.address}")

    def turn_on_light(self):
        asyncio.create_task(self.set_brightness(100))

    def turn_off_light(self):
        asyncio.create_task(self.set_brightness(0))
    
    def toggle_light(self):
        if self.brightness == 0:
            self.turn_on_light()
        else:
            self.turn_off_light()

    async def scan_bluetooth_devices(self):
        print("Scanning for Bluetooth devices...")
        devices = await BleakScanner.discover()
        if devices:
            for i, device in enumerate(devices):
                print(f"{i + 1}: Device: {device.name}, Address: {device.address}")
        else:
            print("No devices found.")
        return devices

    async def choose_device(self, devices):
        if not devices:
            print("No devices available to choose.")
            return None

        try:
            choice = int(input("Choose a device by number: ")) - 1
            if choice not in range(len(devices)):
                print("Invalid choice.")
                return None

            device = devices[choice]
            print(f"Selected device: {device.name} ({device.address})")
            return device
        except ValueError:
            print("Invalid input. Please enter a number.")
            return None

    async def discover_services(self):
        if self.client.is_connected:
            print(f"Connected to {self.address}")
            services = self.client.services
            if services is None:
                await self.client.get_services()  # Ensure services are fetched
                services = self.client.services
            for service in services:
                print(f"Service: {service.uuid}")
                for characteristic in service.characteristics:
                    print(f"  Characteristic: {characteristic.uuid}, Properties: {characteristic.properties}")
                    if "notify" in characteristic.properties:
                        await self.client.start_notify(characteristic.uuid, self.notification_handler)
                        if self.debug:
                            print(f"Started notifications on {characteristic.uuid}")
        else:
            print(f"Failed to connect to {self.address}")

    async def send_test_command(self, characteristic_uuid, command):
        if self.client.is_connected:
            print(f"Connected to {self.address}")
            await self.client.write_gatt_char(characteristic_uuid, command.encode())
            print(f"Command sent to {characteristic_uuid}")
        else:
            print(f"Failed to connect to {self.address}")



async def main(turntable):
    await turntable.initialize_client()
    print("Turntable started turning")
    await turntable.turn_table_cw(90)
    print("Doing other work while the turntable is turning...")
    print("Turntable stopped turning")
    await turntable.turn_table_ccw(90)
    print("Doing other work while the turntable is turning again...")
    await turntable.wait_for_turning_to_stop()
    print("Turntable stopped turning again")

if __name__ == "__main__":
    turntable = Turntable(name="Foldio360", address="D0B15D86-D3B6-9FD0-72C1-EB1BDE03EC2B", debug=False)
    asyncio.run(main(turntable))


