import reflex as rx
from ..components.liveView import liveView, liveViewState
from indb_reflex.states.photoAcquisition_state import PAState
from ..components.focusSlider import focusSliderWithManualControls
from ..photogrammetry.widgets import *


def camera_control_popup() -> rx.Component:
    return rx.dialog.root(
        rx.dialog.trigger(
            rx.box(
                rx.tooltip(
                    rx.flex(
                        rx.button(
                            rx.icon("camera"),
                            size="2",
                            disabled=~PAState.camera_connected,
                            width="100%",
                        ),
                    ),
                    content="Open popup for camera control",
                ),
                width="45%",
            ),
        ),
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("Camera Control"),
                rx.spacer(),
                rx.dialog.close(
                    rx.button(
                        "Close",
                        on_click=liveViewState.stop_capture,
                    ),
                    on_click=liveViewState.stop_capture,
                ),
                width="100%",
                align="center",
            ),
            rx.hstack(
                rx.vstack(
                    liveView(),
                    focusSliderWithManualControls(),
                ),
                rx.divider(orientation="vertical"),
                rx.vstack(
                    rx.hstack(
                        option_input("Aperture", PAState.camera_aperture, PAState.aperture_values, PAState.set_camera_aperture, width="100px"),
                        option_input("ISO", PAState.camera_iso, PAState.iso_values, PAState.set_camera_iso, width="100px"),
                        option_input("Shutter", PAState.camera_shutter_speed, PAState.shutter_speeds, PAState.set_camera_shutter_speed, width="100px"),
                    ),
                ),
            ),
            
            width="100%",
            style={"max-width": "90vw"},  # Set the maximum width to 90% of the viewport width
        ),
        style={"max-width": "90vw"},  # Set the maximum width to 90% of the viewport width
    )