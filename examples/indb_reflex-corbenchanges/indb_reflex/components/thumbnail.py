from ..popups.thumbnailCapturePopup import thumbnail_capture_popup
import reflex as rx
import os


def thumbnail_image_with_capture(state:rx.State) -> rx.Component:
    return rx.fragment(
        # rx.script(f"""
        #     console.log("Thumbnail component received model ID:", "{id}"); 
        # """),
        rx.fragment(
            rx.cond(
                state.thumbnail_exists,
                rx.image(
                    src=state.thumbnail_location_string,
                    width="100%",
                    height="auto",
                    flex_grow="1",
                    flex_shrink="1"
                ),
                rx.box(
                    rx.upload(
                        rx.vstack(
                            rx.text("Drag and Drop Image"),
                            rx.text("--- Or ---", padding_top="6px", padding_bottom="6px"),
                            rx.button("Select Image"),
                            align="center"
                        ),
                        id="thumbnail_upload",
                        max_files=1,
                        accept={
                            "image/jpeg": [".jpg", ".jpeg"],
                        },
                        on_drop=state.save_thumbnail(rx.upload_files(upload_id="thumbnail_upload")),
                    ),
                    rx.hstack(
                        rx.spacer(),
                        rx.text("--- Or ---"),
                        rx.spacer(),
                        padding_top="6px",
                        width="100%",
                    ),
                    rx.hstack(
                        rx.spacer(),
                        thumbnail_capture_popup(),
                        rx.spacer(),
                        padding_top="6px",
                        width="100%",
                    ),
                    width="100%",
                ),
            ),
        ),
    )



