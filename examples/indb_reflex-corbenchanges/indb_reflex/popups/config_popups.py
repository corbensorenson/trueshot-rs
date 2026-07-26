import reflex as rx
from indb_reflex.states.photoAcquisition_state import PAState
from .thumbnailCapturePopup import thumbnail_capture_popup
from ..states.user_state import *


def camera_config_popup() -> rx.Component:
        return rx.dialog.root(
                rx.dialog.trigger(
                    rx.box(
                        rx.tooltip(
                            rx.button(
                                rx.icon("settings"),
                                size="2",
                                backdrop_filter="blur(2px)",
                                background_color="rgba(255, 255, 255, 0.00)",
                            ),
                            content="Camera Config",
                        ),
                    ),
                ),
                rx.dialog.content(
                    rx.dialog.title("Camera Config:"),
                    rx.hstack(
                        rx.text("Current Selected Camera: "),
                        rx.select(user_state.camera_names, default_value=user_state.current_camera_name, on_change=user_state.set_current_camera),
                    ),
                    rx.cond(
                        user_state.camera_chosen,
                        rx.hstack(
                            rx.text("Implementation: "),
                            rx.input(default_value=user_state.current_camera.implementation),

                        ),
                    ),

                    rx.dialog.close(
                        rx.button(
                            "Close",
                        ),
                    ),
                ),
            )

def turntable_config_popup() -> rx.Component:
        return rx.dialog.root(
                rx.dialog.trigger(
                    rx.box(
                        rx.tooltip(
                            rx.button(
                                rx.icon("settings"),
                                size="2",
                                backdrop_filter="blur(2px)",
                                background_color="rgba(255, 255, 255, 0.00)",
                            ),
                            content="Turntable Config",
                        ),
                    ),
                ),
                rx.dialog.content(
                    rx.dialog.title("Turntable Config:"),
                    rx.hstack(
                        rx.text("Current Selected turntable: "),
                        rx.select(user_state.turntable_names, default_value=user_state.current_turntable_name, on_change=user_state.set_current_turntable),
                    ),
                    rx.cond(
                        user_state.turntable_chosen,
                        rx.hstack(
                            rx.text("Implementation: "),
                            rx.input(default_value=user_state.current_turntable.implementation),
                            
                        ),
                    ),


                    rx.dialog.close(
                        rx.button(
                            "Close",
                        ),
                    ),
                ),
            )

def arm_config_popup() -> rx.Component:
        return rx.dialog.root(
                rx.dialog.trigger(
                    rx.box(
                        rx.tooltip(
                            rx.button(
                                rx.icon("settings"),
                                size="2",
                                backdrop_filter="blur(2px)",
                                background_color="rgba(255, 255, 255, 0.00)",
                            ),
                            content="Arm Config",
                        ),
                    ),
                ),
                rx.dialog.content(
                    rx.dialog.title("Arm Config:"),
                    rx.hstack(
                        rx.text("Current Selected arm: "),
                        rx.select(user_state.arm_names, default_value=user_state.current_arm_name, on_change=user_state.set_current_arm),
                    ),
                    rx.cond(
                        user_state.arm_chosen,
                        rx.hstack(
                            rx.text("Implementation: "),
                            rx.input(default_value=user_state.current_arm.implementation),
                            
                        ),
                    ),

                    rx.dialog.close(
                        rx.button(
                            "Close",
                        ),
                    ),
                ),
            )

def edit_current_model_popup() -> rx.Component:
        return rx.dialog.root(
                rx.dialog.trigger(
                    rx.box(
                        rx.tooltip(
                            rx.button(
                                rx.icon("pencil"),
                            ),
                            content="Edit Model Settings",
                        ),
                    ),
                ),
                rx.dialog.content(
                    rx.dialog.title("Model settings:"),
                    rx.hstack(
                        rx.text("New Name: "),
                        rx.input(),
                        rx.button("Save"),
                    ),

                    rx.hstack(
                        rx.text("Capture new thumbnail"),
                        rx.cond(
                            ~PAState.show_thumbnail_section_in_popop,
                            rx.tooltip(rx.icon("arrow-down", on_click=PAState.toggle_show_thumbnail_section), content="Show"),
                            rx.tooltip(rx.icon("arrow-up", on_click=PAState.toggle_show_thumbnail_section), content="Hide"),
                        ),
                    ),
                    rx.cond(
                            PAState.show_thumbnail_section_in_popop,
                            rx.box(
                                rx.upload(
                                    rx.vstack(
                                        rx.text("Drag and Drop Image"),
                                        rx.text("--- Or ---", padding_top="6px", padding_bottom="6px"),
                                        rx.button("Select Image"),
                                        align="center"
                                    ),
                                    id="upload1",
                                    max_files=1,  # Limit to single file
                                    accept={
                                        "image/jpeg": [".jpg", ".jpeg"],
                                    },
                                    on_drop=PAState.save_thumbnail(rx.upload_files(upload_id="upload1")),
                                ),
                                rx.hstack(
                                    rx.spacer(),
                                    rx.text("--- Or ---"),
                                    rx.spacer(),
                                    padding_top = "6px",
                                    width = "100%",
                                ),
                                rx.hstack(
                                    rx.spacer(),
                                    thumbnail_capture_popup(),
                                    rx.spacer(),
                                    padding_top = "6px",
                                    width = "100%",
                                ),
                                
                                width = "100%",
                            ),
                    ),

                    
                    rx.dialog.close(
                        rx.button(
                            "Close",
                        ),
                    ),
                ),
            )

def edit_current_sequence_popup() -> rx.Component:
        return rx.dialog.root(
                rx.dialog.trigger(
                    rx.box(
                        rx.tooltip(
                            rx.button(
                                rx.icon("pencil"),
                            ),
                            content="Edit Sequence Settings",
                        ),
                    ),
                ),
                rx.dialog.content(
                    rx.dialog.title("Sequence settings:"),
                    rx.hstack(
                        rx.text("Many of the settings are bellow this area, here is what is left:"),
                    ),

                    rx.dialog.close(
                        rx.button(
                            "Close",
                        ),
                    ),
                ),
            )

