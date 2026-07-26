import reflex as rx
from ..components.liveView import liveView, liveViewState
from indb_reflex.states.photoAcquisition_state import PAState


def calibrate_lens_start_and_end() -> rx.Component:
        return rx.dialog.root(
                rx.dialog.trigger(
                    rx.box(  # Add a box wrapper here
                        rx.tooltip(
                            rx.button(
                                rx.icon("scan-eye"),
                                size="2",
                                backdrop_filter="blur(2px)",
                                background_color="rgba(255, 255, 255, 0.00)",
                            ),
                            content="Calibrate lens start and end",
                        ),
                    ),
                ),
                rx.dialog.content(
                    rx.dialog.title("Calibrate lens start and end"),
                    rx.vstack(
                        rx.text("Ensure camera is in manual focus mode and focus is backed all the way up physically. We are going to have it step forward one at a time until we hit an error."),
                        liveView(),
                    ),
                    rx.hstack(
                        rx.button("start test", on_click=PAState.perform_focus_limit_test()),
                        rx.dialog.close(
                            rx.button(
                                "Cancel",
                                on_click=liveViewState.stop_capture,
                            ),
                        ),
                    ),
                ),
            )