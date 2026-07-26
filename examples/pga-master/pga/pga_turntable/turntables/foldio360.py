from  .bt_turntable import BTTurntable
import asyncio

class Foldio360(BTTurntable):
    def __init__(self, **kwargs):
        super().__init__(name="Foldio360", characteristic_uuid="6e400002-b5a3-f393-e0a9-e50e24dcca9e",
                          notify_characteristic_uuid="6e400003-b5a3-f393-e0a9-e50e24dcca9e", **kwargs)

    async def rotate(self, degrees, speed=3, wait=True):
        # print(f"Foldio360: Rotating by {degrees} degrees")
        if self.client is None or not self.client.is_connected:
            await self.reconnect_async()
        dir="CW" if degrees > 0 else "CCW"
        degrees=abs(degrees)
        command = f"rotate({dir},{degrees},{speed},1)".encode()
        # print(f"Foldio360: Sending command: {command}")
        self.turning_event.clear()
        await self.client.write_gatt_char(self.characteristic_uuid, command, response=True)
        if wait: await self.turning_event.wait()

    async def turn_sync(self, angle, speed, isBlocking=True):
        await self.rotate(angle, speed, isBlocking)

    def rotate_sync(self, degrees, speed=3, wait = True):
        self.loop.create_task(self.turn_sync(degrees, speed, wait))

    async def disconnect(self):
        await asyncio.sleep(1)
        