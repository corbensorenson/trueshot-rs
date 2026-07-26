import reflex as rx
from ..components.liveView import liveView, liveViewState
from indb_reflex.states.photoAcquisition_state import PAState
from ..components.focusSlider import focusSliderWithManualControls
from ..photogrammetry.widgets import *


def thumbnail_capture_popup() -> rx.Component:
        return rx.dialog.root(
                rx.dialog.trigger(
                    rx.box(  # Add a box wrapper here
                        rx.tooltip(
                            rx.flex(
                                rx.button(
                                    rx.icon("camera"),
                                    size="2",
                                    disabled=~PAState.camera_connected,
                                    width = "100px",
                                ),  
                            ),
                            content="Open popup for thumbnail capture",
                        ),
                    ),
                ),
                rx.dialog.content(
                    rx.dialog.title("Take a thumbnail image"),
                    rx.vstack(
                        liveView(),
                        focusSliderWithManualControls(),
                    ),
                    rx.hstack(
                        option_input("Aperture", PAState.camera_aperture, PAState.aperture_values, PAState.set_camera_aperture, width="100px"),
                        option_input("ISO", PAState.camera_iso, PAState.iso_values, PAState.set_camera_iso, width="100px"),
                        option_input("Shutter", PAState.camera_shutter_speed, PAState.shutter_speeds, PAState.set_camera_shutter_speed, width="100px"),
                        rx.dialog.close(
                            rx.tooltip(rx.flex(rx.button(rx.icon("camera"), on_click=PAState.capture_thumbnail),), content="Capture Thumbnail Image"),
                            rx.spacer(),
                            rx.button(
                                "Cancel",
                                on_click=liveViewState.stop_capture,
                            ),  
                            on_click=liveViewState.stop_capture,
                            width="100%"
                        ),
                    ),
                ),
            )