import time
import reflex as rx
from reflex.state import State
from pga.pga_camera.camera import Camera
import asyncio
from ..states import photoAcquisition_state as PAState
import gphoto2


class liveViewState(rx.State):
    sample_rate: int = 500  # Default sample rate in milliseconds
    capturing: bool = False
    _timestamp: int = 0  # Backend-only var to track updates
    rx.get_upload_url("live.jpg") #do this once for some reason

    @rx.var
    def current_image_url(self) -> str:
        # Use computed var to generate the URL with timestamp
        base_url = "http://localhost:8000/_upload/live.jpg"  # Direct path to upload directory
        return f"{base_url}?t={self._timestamp}"

    @rx.event(background=True)
    async def initialize_camera(self):
        if not Camera.connected:
            await Camera.connect()  # Await the coroutine

    def start_capture(self):
        if not Camera.connected:
            asyncio.create_task(self.initialize_camera())  # Start the coroutine in the background
        self.capturing = True
        #asyncio.create_task(self._run_capture_photos())  # Start the coroutine in the background
        

    def stop_capture(self):
        self.capturing = False

    @rx.event
    async def update_image(self, _):
        """Event handler called by moment to update image"""
        if self.capturing:
            outfile = rx.get_upload_dir() / "live.jpg"
            for _ in range(3):  # Retry up to 3 times
                try:
                    Camera.camera.capture_preview(str(outfile))
                    self._timestamp = int(time.time() * 1000)
                    break
                except gphoto2.GPhoto2Error as e:
                    if e.code == -110:  # I/O in progress
                        await asyncio.sleep(0.5)  # Wait for 500ms before retrying
                    else:
                        raise e
        
    def set_sample_rate(self, rate):
        self.sample_rate = rate
        if self.capturing:
            self.stop_capture()
            self.start_capture()  # Restart capture with the new sample rate
        return rx.toast.success("Sample rate changed to " + rate)

def liveImage() -> rx.Component:
    return rx.vstack(
        rx.image(
            src=liveViewState.current_image_url,
            alt="Live View",
            width="100%"
        ),
        rx.moment(
            interval=liveViewState.sample_rate,
            on_change=liveViewState.update_image,
            display="none",
        ),
    )

def liveView() -> rx.Component:
    return rx.box(
        liveImage(),
        # Start/Stop button overlay with tooltip
        rx.box(
            startStopButton(),
            position="absolute",
            top="5px",
            left="5px",
        ),
        # Settings dialog with tooltip
        rx.dialog.root(
            rx.dialog.trigger(
                rx.box(  # Add a box wrapper here
                    rx.tooltip(
                        rx.button(
                            rx.icon("settings"),
                            size="3",
                            backdrop_filter="blur(2px)",
                            background_color="rgba(255, 255, 255, 0.00)",
                        ),
                        content="Adjust capture settings",
                    ),
                    position="absolute",
                    top="5px",
                    right="5px",
                ),
            ),
            rx.dialog.content(
                rx.dialog.title("Settings"),
                rx.hstack(
                    rx.spacer(),
                    rx.text("Sample rate (ms):", margin_right="10px"),
                    rx.input(
                        type="number",
                        value=liveViewState.sample_rate,
                        on_change=lambda e: liveViewState.set_sample_rate(e), 
                        placeholder="Sample rate (ms)",
                        margin_right="10px",
                        width="100px",
                    ),
                    rx.spacer(),
                ),
                rx.dialog.close(
                    rx.button("Close"),
                ),
            ),
        ),
        position="relative",
    )


def startStopButton() -> rx.Component:
    return rx.cond(
        liveViewState.capturing,
        rx.tooltip(
            rx.box(
                rx.button(
                    rx.icon("octagon-pause"), 
                    on_click=liveViewState.stop_capture,
                    backdrop_filter="blur(2px)",
                    background_color="rgba(255, 255, 255, 0.00)",
                ),
            ),
            content="Stop live view capture",
        ),
        rx.tooltip(
            rx.box(
                rx.button(
                    rx.icon("play"), 
                    on_click=liveViewState.start_capture,
                    backdrop_filter="blur(2px)",
                    background_color="rgba(255, 255, 255, 0.00)",
                ),
            ),
            content="Start live view capture",
        ),
    )