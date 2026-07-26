from ..components.liveView import liveView, liveViewState
import reflex as rx
from ..components.focusSlider import focusSlider, focusSliderWithManualControls 
from indb_reflex.states.photoAcquisition_state import PAState


def focus_stack_popup() -> rx.Component:
    return rx.dialog.root(
        rx.dialog.trigger(rx.tooltip(rx.button(rx.icon('layout-grid'), disabled=~PAState.camera_connected), content = "Focus Calibration")),
        rx.dialog.content(
            rx.text("Ensure camera is in manual focus mode and focus is set to the nearest point of interest."),
            rx.text("object should be aligned to be elongated along the z-axis."),
            rx.text("Ensure backstop for focus stack is in place for calibrating end point."),
            rx.hstack(
                rx.input(value=PAState.focus_steps, type="number", on_change=PAState.set_focus_steps),
                focus_stack_test_image_Popup(),
                rx.dialog.close(
                    rx.button("Close", size="2", on_click=liveViewState.stop_capture),
                ),
            ),
            liveView(),
            focusSliderWithManualControls(),
        ),
    )


def focus_stack_test_image_Popup() -> rx.Component:
    return rx.dialog.root(
        rx.dialog.trigger(
            rx.tooltip(
                rx.button(
                    "start focus stack test sample",
                    on_click=PAState.load_focus_stack_test_images,
                ),
                content="opens a popup with test shots in it"
            )
        ),
        rx.dialog.content(
            rx.cond(
                PAState.started_taking_test_shots,
                rx.text("Taking test shots..."),
                rx.grid(
                    rx.foreach(
                        PAState.test_pics,
                        lambda i, image: rx.box(
                            rx.image(src=image, alt=f"testImage", width="100%"),
                            border="1px solid black",
                            padding="10px",
                            cursor="pointer",
                        ),
                    ),
                    template_columns="repeat(3, 1fr)",
                    gap="10px",
                    padding="20px",
                ),
            ),
            rx.dialog.close(
                rx.button("Close", size="3", on_click=PAState.clear_test_pics),
            ),
        ),
    )