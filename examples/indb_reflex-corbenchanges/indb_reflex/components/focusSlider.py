import json
from typing import Any
import reflex as rx
from indb_reflex.states.focusSliderState import focusSliderState
from indb_reflex.states.photoAcquisition_state import PAState
from ..popups.lensCalibrationPopup import calibrate_lens_start_and_end

def track() -> rx.Component:
    return rx.box(
        background_color="rgba(255, 255, 255, 0.3)",
        height="4px",
        width="100%", 
        position="absolute",
        top="50%",
        transform="translateY(-50%)",
    )

def trackFill() -> rx.Component:
    return rx.box(
        background_color="rgba(255, 255, 255, 0.6)",
        height="4px",
        position="absolute",
        top="50%",
        transform="translateY(-50%)",
        left=focusSliderState.photo_stack_start_end_coverage_percent_left_offset,
        width=focusSliderState.photo_stack_start_end_coverage_percent_width,
    )

def startBox() -> rx.Component:
    return rx.box(
        rx.tooltip(
            rx.box(
                background_color="red",
                width="10px",
                height="10px",
                position="absolute",
                left=focusSliderState.photo_stack_start_end_coverage_percent_left_offset,
                top="50%",
                transform="translate(-50%, -50%)",
                cursor="pointer",
                tooltip="Start",
                on_click=PAState.jump_to_focus_start,
            ),
            content="Jump to start of photo stack",
        )
    )

def endBox() -> rx.Component:
    return rx.box(
        rx.tooltip(
            rx.box(
                background_color="blue",
                width="12px",
                height="12px",
                position="absolute",
                left=f"calc({focusSliderState.photo_stack_start_end_coverage_percent_left_offset} + {focusSliderState.photo_stack_start_end_coverage_percent_width})",
                top="50%",
                transform="translate(-50%, -50%)",
                cursor="pointer",
                tooltip="End",
                on_click=PAState.jump_to_focus_end,
            ),
            content="jump to end of photo stack",
        )
    )
    
def draggableHandle() -> rx.Component:
    return rx.box(
        rx.tooltip(
            rx.box(
            background_color="white",
            width="20px",
            height="20px",
            border_radius="50%",
            position="absolute",
            left=focusSliderState.current_percent,
            top="50%",
            transform="translate(-50%, -50%)",
            cursor="pointer",
            z_index="1",
            on_mouse_down=focusSliderState.start_drag,
            on_mouse_up=focusSliderState.end_drag,
            on_mouse_move=focusSliderState.handle_drag_js_test.throttle(16),
            capture_event=True
        ),
        content="Drag to adjust focus",
        ),
    )

def set_start_focus_button() -> rx.Component:
    return rx.tooltip(
            rx.button(
                rx.icon("chart-no-axes-column-increasing"),
                on_click=PAState.set_start_focus_to_current,
                background_color="rgba(255, 255, 255, 0.00)",
                size="2",
            ),
            content="Set start focus to current focus",
        ),

def set_end_focus_button() -> rx.Component:
    return rx.tooltip(
            rx.button(
                rx.icon("chart-no-axes-column-decreasing"),
                on_click=PAState.set_end_focus_to_current,
                background_color="rgba(255, 255, 255, 0.00)",
                size="2",
            ),
            content="Set end focus to current focus",
        ),

def move_focus_forward_button_low() -> rx.Component:
    return rx.tooltip(
        rx.button(
        rx.icon("arrow-right"),
        on_click=PAState.move_camera_focus(focusSliderState.manual_low),
        background_color="rgba(255, 255, 255, 0.00)",
        ),
        content="Move focus forward low",
    )
def move_focus_backward_button_low() -> rx.Component:
    return rx.tooltip(
        rx.button(
        rx.icon("arrow-left"),
        on_click=PAState.move_camera_focus(-focusSliderState.manual_low),
        background_color="rgba(255, 255, 255, 0.00)",
        ),
        content="Move focus backward low",
    )

def move_focus_forward_button_mid() -> rx.Component:
    return rx.tooltip(
        rx.button(
        rx.icon("fast-forward"),
        on_click=PAState.move_camera_focus(focusSliderState.manual_mid),
        background_color="rgba(255, 255, 255, 0.00)",
        ),
        content="Move focus forward mid",
    )

def move_focus_backward_button_mid() -> rx.Component:
    return rx.tooltip(
        rx.button(
        rx.icon("rewind"),
        on_click=PAState.move_camera_focus(-focusSliderState.manual_mid),
        background_color="rgba(255, 255, 255, 0.00)",
        ),
        content="Move focus backward mid",
    )

def move_focus_forward_button_high() -> rx.Component:
    return rx.tooltip(
        rx.button(
        rx.icon("skip-forward"),
        on_click=PAState.move_camera_focus(focusSliderState.manual_high),
        background_color="rgba(255, 255, 255, 0.00)",
    ),
    content="Move focus forward high",
    )

def move_focus_backward_button_high() -> rx.Component:
    return rx.tooltip(
        rx.button(
        rx.icon("skip-back"),
        on_click=PAState.move_camera_focus(-focusSliderState.manual_high),
        background_color="rgba(255, 255, 255, 0.00)",
    ),
    content="Move focus backward high",
    )

def current_focus_input() -> rx.Component:
    return rx.input(
            value=PAState.supposed_focus_z,
            type="number",
            on_change=PAState.set_current_focus_z,
            min_=0,
            max_=PAState.lens_end_focus_z,
            width="48px",
            id="slider-input",
        )

def manual_focus_buttons_settings() -> rx.Component:
    #dialogue button set up here
    return rx.dialog.root(
            rx.dialog.trigger(
                rx.box(  # Add a box wrapper here
                    rx.tooltip(
                        rx.button(
                            rx.icon("sliders-horizontal"),
                            size="2",
                            backdrop_filter="blur(2px)",
                            background_color="rgba(255, 255, 255, 0.00)",
                        ),
                        content="Adjust button values",
                    ),
                ),
            ),
            rx.dialog.content(
                rx.dialog.title("Settings"),
                rx.vstack(
                    rx.hstack(
                        rx.spacer(),
                        rx.text("low amount:", margin_right="10px"),
                        rx.input(
                            type="number",
                            value=focusSliderState.manual_low,
                            on_change=lambda e: focusSliderState.set_manual_low_incriment(e), 
                            placeholder="low incriment",
                            margin_right="10px",
                            width="100px",
                        ),
                        rx.spacer(),
                    ),
                    rx.hstack(
                        rx.spacer(),
                        rx.text("mid amount:", margin_right="10px"),
                        rx.input(
                            type="number",
                            value=focusSliderState.manual_mid,
                            on_change=lambda e: focusSliderState.set_manual_mid_incriment(e), 
                            placeholder="mid incriment",
                            margin_right="10px",
                            width="100px",
                        ),
                        rx.spacer(),
                    ),
                    rx.hstack(
                        rx.spacer(),
                        rx.text("high amount:", margin_right="10px"),
                        rx.input(
                            type="number",
                            value=focusSliderState.manual_high,
                            on_change=lambda e: focusSliderState.set_manual_high_incriment(e), 
                            placeholder="high incriment",
                            margin_right="10px",
                            width="100px",
                        ),
                        rx.spacer(),
                    ),
                ),
                rx.dialog.close(
                    rx.button("Close"),
                ),
            ),
        )


def manual_focus_buttons() -> rx.Component:
    return rx.hstack(
        calibrate_lens_start_and_end(),
        rx.spacer(),
        move_focus_backward_button_high(),
        move_focus_backward_button_mid(),
        move_focus_backward_button_low(),
        current_focus_input(),
        move_focus_forward_button_low(),
        move_focus_forward_button_mid(),
        move_focus_forward_button_high(),
        rx.spacer(),
        manual_focus_buttons_settings(),
        spacing="1",
        width="100%",
    )
    

def focusSlider() -> rx.Component:
    return rx.hstack(
        set_start_focus_button(),
        rx.box(
            track(),
            trackFill(),
            startBox(),
            endBox(),
            draggableHandle(),
            position="relative",
            width="100%",
            height="40px",
            id="slider",  # Important: moved ID here
        ),
        set_end_focus_button(),
        width="100%",
    )

def focusSliderWithManualControls() -> rx.Component:
    return rx.vstack(
        focusSlider(),
        manual_focus_buttons(),
        spacing="1",
        width="100%",
    )
