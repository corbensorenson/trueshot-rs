import reflex as rx
from indb_reflex.states.photoAcquisition_state import PAState


#max number of selected images set to 0 means no limit
def settingImagePopup(setting, default_values, toolTipMessage, icon_name = 'layout-grid', maxNumberSelected = 0, on_close = PAState.clear_selected_images) -> rx.Component:
    return rx.dialog.root(
        rx.dialog.trigger(rx.tooltip(rx.button(rx.icon(icon_name), on_click=PAState.take_pictures_with_settings(setting, default_values), disabled=~PAState.camera_connected), content=toolTipMessage)),
        rx.dialog.content(
            rx.cond(
                PAState.started_taking_test_shots,
                rx.text("Taking test shots..."),
                create_image_grid(maxNumberSelected),
            ),  
            rx.dialog.close(
                rx.button("Close", size="3", on_click=on_close),
            ),
        ),
    )

def create_image_grid(maxNumberSelected) -> rx.Component:
    # Use rx.cond to check if PAState.test_pics is empty
    return rx.cond(
        PAState.test_pics == [],
        rx.text("No images available.", text_align="center", padding="20px"),
        rx.grid(
            rx.foreach(
                # PAState.test_pics is a list of tuples with the following format: (image_path, setting, value)
                PAState.test_pics,
                lambda i, image: (
                    rx.box(
                        rx.image(src=image[0], alt=f"{image[1]}: {image[2]}", width="100%"),
                        rx.text(image[2], text_align="center"),
                        border="1px solid black",
                        padding="10px",
                        cursor="pointer",
                        on_click=lambda setting=image[1], value=image[2]: PAState.toggle_image_as_selected(setting, value, maxNumberSelected),
                    ) if isinstance(image, (list, tuple)) and len(image) == 3 else rx.text("Invalid image data", text_align="center")
                )
            ),
            template_columns="repeat(3, 1fr)",
            gap="10px",
            padding="20px",
        )
    )