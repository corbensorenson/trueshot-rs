from ..templates import template
import reflex as rx
from reflex_ag_grid import ag_grid
from ..states.devices_state import devices_table_state
from ..components.search_bar import search_bar
from ..components.ag_grid_baseTable import selectable_ag_table




def device_search_bar() -> rx.Component:
    return rx.hstack(
        search_bar(devices_table_state),
        rx.select(devices_table_state.device_categories, value=devices_table_state.current_category, on_change=devices_table_state.set_current_category)
    )

def devices_table() -> rx.Component:
    col_defs = [
        ag_grid.column_def(field="name", header_name="Device Name", editable=True),
        ag_grid.column_def(field="category", header_name="category", editable=True),
        ag_grid.column_def(field="description", header_name="Description", editable=True),
    ]
    return selectable_ag_table("Devices table",col_defs, devices_table_state)

def device_section() -> rx.Component:
    return rx.card(
            rx.vstack(
                rx.center(
                    rx.spacer(),
                    rx.icon("camera"),
                    rx.heading("Devices", size="5", margin_left="5px"),
                    rx.spacer(),
                    rx.hstack(
                        addDevice(),
                        spacing="2",
                    ),
                    width="100%",
                ),
                device_search_bar(),
                devices_table(),
            ),
            width="100%",
        ),


def device_info_card() -> rx.Component:
    def show_data(item):
        return rx.table.row(
            rx.table.cell(item[0]),
            rx.table.cell(item[1])
        )

    def data_table():
        return rx.table.root(
            rx.table.header(
                rx.table.row(
                    rx.table.column_header_cell("Key"),
                    rx.table.column_header_cell("Value"),
                ),
            ),
            rx.table.body(
                rx.foreach(devices_table_state.device_config_data, show_data)
            ),
            width="100%",
        )

    return rx.card(
        rx.cond(
            ~devices_table_state.has_selected_device,
            rx.center(
                rx.hstack(
                    rx.icon("shield-alert"),
                    rx.heading("No Device Selected", size="4"),
                ),
                width="100%",
                align="center",
            ),
            rx.vstack(
                rx.hstack(
                    rx.heading("Device Information", size="6"),
                    rx.spacer(),
                    rx.tooltip(rx.flex(delete_device_popup_button()), content=f"Delete {devices_table_state.selected_device.name}"),
                    rx.tooltip(rx.button(rx.icon("layers-2"), size="3", on_click=devices_table_state.copy_device(devices_table_state.selected_device_id)),content=f"Create Copy of {devices_table_state.selected_device.name}"),
                    rx.cond(
                        devices_table_state.editable,
                        rx.tooltip(rx.button(rx.icon("ban"), size="3", on_click=devices_table_state.toggle_editable),content="Cancel Editing"),
                        rx.tooltip(rx.button(rx.icon("pencil"), size="3", on_click=devices_table_state.toggle_editable),content="Edit Device"),
                    ),
                    width="100%",
                    align="center",
                ),
                rx.form(
                    rx.vstack(
                        rx.vstack(
                            rx.heading("Device Name: ", size="4", padding_bottom="3px", margin_bottom="0px"),
                            rx.cond(
                                devices_table_state.editable,
                                rx.input(
                                    placeholder="Device Name",
                                    name="name",
                                    default_value=devices_table_state.selected_device.name,
                                    width="100%",
                                ),
                                rx.text(devices_table_state.selected_device.name),
                            ),
                            width="100%",
                            spacing="0",
                        ),
                        rx.vstack(
                            rx.heading("Device Description: ", size="4", padding_bottom="3px", margin_bottom="0px"),
                            rx.cond(
                                devices_table_state.editable,
                                rx.input(
                                    placeholder="Device Description",
                                    name="description",
                                    default_value=devices_table_state.selected_device.description,
                                    width="100%",
                                ),
                                rx.text(devices_table_state.selected_device.description),
                            ),
                            width="100%",
                            spacing="0",
                        ),
                        rx.vstack(
                            rx.heading("Device Category: ", size="4", padding_bottom="3px", margin_bottom="0px"),
                            rx.cond(
                                devices_table_state.editable,
                                rx.select(
                                    devices_table_state.device_categories,
                                    placeholder="Category",
                                    name="category",
                                    default_value=devices_table_state.selected_device.category,
                                    width="100%",
                                ),
                                rx.text(devices_table_state.selected_device.category),
                            ),
                            width="100%",
                            spacing="0",
                        ),
                        rx.vstack(
                            rx.heading("Device Implementation: ", size="4", padding_bottom="3px", margin_bottom="0px"),
                            rx.cond(
                                devices_table_state.editable,
                                rx.input(
                                    placeholder="Device Implementation",
                                    name="implementation",
                                    default_value=devices_table_state.selected_device.implementation,
                                    width="100%",
                                ),
                                rx.text(devices_table_state.selected_device.implementation),
                            ),
                            width="100%",
                            spacing="0",
                        ),
                        rx.vstack(
                            rx.heading("Additional Information: ", size="4", padding_bottom="3px", margin_bottom="0px"),
                            rx.cond(
                                devices_table_state.editable,
                                rx.input(
                                    placeholder="Device Notes",
                                    name="notes",
                                    default_value=devices_table_state.selected_device.notes,
                                    width="100%",
                                ),
                                rx.text(devices_table_state.selected_device.notes),
                            ),
                            width="100%",
                            spacing="0",
                        ),
                        rx.vstack(
                            rx.heading("Device Config: ", size="4", padding_bottom="3px", margin_bottom="0px"),
                            rx.cond(
                                devices_table_state.editable,
                                rx.text_area(
                                    default_value=devices_table_state.config_data_string,
                                    name="config",
                                    width="100%",
                                ),
                                data_table(),
                            ),
                            width="100%",
                            spacing="0",
                        ),
                        rx.cond(
                            devices_table_state.editable,
                            rx.hstack(
                                rx.spacer(),
                                rx.button(
                                    "Save Changes",
                                    type="submit",
                                ),
                                spacing="3",
                                justify="end",
                                width="100%",
                            ),
                        ),
                        width="100%",
                    ),
                    on_submit=lambda form_data: devices_table_state.update_device(form_data),
                    reset_on_submit=False,
                ),
                spacing="3",
            ),
        ),
        width="100%",
    )

def delete_device_popup_button() -> rx.Component:
    return rx.dialog.root(
        rx.dialog.trigger(
            rx.button(rx.icon("trash"), size="3")
        ),
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("Delete Device", padding_top="12", size="5"),
                rx.spacer(),
                rx.dialog.close(
                    rx.button("X", size="2", border="0"),
                ),
            ),
            rx.hstack(
                rx.text("Device Name: "),
                rx.text(devices_table_state.selected_device.name),
            ),
            rx.hstack(
                rx.text("Device Description: "),
                rx.text(devices_table_state.selected_device.description),
            ),
            rx.hstack(
                rx.text("Confirm Delete: "),
                rx.dialog.close(
                    rx.button("Cancel"),
                    rx.button("Delete", on_click=devices_table_state.delete_device(devices_table_state.selected_device_id)),
                ),
            ),
            max_width="450px",
        ),
    )

def addDevice() -> rx.Component:
    """The add device page with a form."""
    return rx.dialog.root(
        rx.dialog.trigger(
            rx.tooltip(rx.button(rx.icon("square-plus")), content="Add Device")
        ),
        rx.dialog.content(
            rx.dialog.title("Add a new Device"),
            rx.dialog.description("Fill in the device details"),
            rx.form(
                rx.vstack(
                    rx.hstack(
                        rx.heading("Name: ", size="4"),
                        rx.input(
                            placeholder="Device Name",
                            name="name"
                        ),
                        width="100%",
                        align="center",
                    ),
                    rx.hstack(
                        rx.heading("Category: ", size="4"),
                        rx.select(
                            devices_table_state.device_categories,
                            placeholder="Category",
                            name="category"
                        ),
                        width="100%",
                        align="center",
                    ),
                    rx.hstack(
                        rx.heading("Implementation: ", size="4"),
                        rx.input(
                            placeholder="Device Implementation", 
                            name="implementation"
                        ),
                        width="100%",
                        align="center",
                    ),
                    rx.hstack(
                        rx.heading("Description: ", size="4"),
                        rx.input(
                            placeholder="Device Description",
                            name="description"
                        ),
                        width="100%",
                        align="center",
                    ),
                    rx.hstack(
                        rx.heading("Notes: ", size="4"),
                        rx.input(
                            placeholder="Device Notes",
                            name="notes"
                        ),
                        width="100%",
                        align="center",
                    ),
                    rx.hstack(
                        rx.dialog.close(
                            rx.button(
                                "Cancel",
                                variant="soft",
                                color_scheme="gray",
                            ),
                        ),
                        rx.button(
                            "Add Device", 
                            type="submit"
                        ),
                        spacing="3",
                        justify="end",
                    ),
                    width="100%"
                ),
                on_submit=devices_table_state.add_new_device,
                reset_on_submit=True,
            ),
        ),
    )


def edit_device_section() -> rx.Component:
    """Dialog for editing an existing device."""
    return rx.form(
            rx.vstack(
                rx.hstack(
                    rx.heading("Name: ", size="4"),
                    rx.input(
                        placeholder="Device Name",
                        name="name",
                        default_value=devices_table_state.selected_device.name,
                        width="100%",
                    ),
                    width="100%",
                    align="center",
                ),
                rx.hstack(
                    rx.heading("Category: ", size="4"),
                    rx.select(
                        devices_table_state.device_categories,
                        placeholder="Category",
                        name="category",
                        default_value=devices_table_state.selected_device.category,
                        width="100%",
                    ),
                    width="100%",
                    align="center",
                ),
                rx.hstack(
                    rx.heading("Implementation: ", size="4"),
                    rx.input(
                        placeholder="Device Implementation",
                        name="implementation",
                        default_value=devices_table_state.selected_device.implementation,
                        width="100%",
                    ),
                    width="100%",
                    align="center",
                ),
                rx.hstack(
                    rx.heading("Description: ", size="4"),
                    rx.input(
                        placeholder="Device Description",
                        name="description",
                        default_value=devices_table_state.selected_device.description,
                        width="100%",
                    ),
                    width="100%",
                    align="center",
                ),
                rx.hstack(
                    rx.heading("Notes: ", size="4"),
                    rx.input(
                        placeholder="Device Notes",
                        name="notes",
                        default_value=devices_table_state.selected_device.notes,
                        width="100%",
                    ),
                    width="100%",
                    align="center",
                ),
                rx.hstack(
                    rx.heading("Configuration: ", size="4"),
                    rx.text_area(
                        default_value=devices_table_state.device_config_data,
                        name="config",
                        editable=True,
                        width="100%",
                    ),
                    width="100%",
                    align="center",
                ),
                rx.hstack(
                    rx.spacer(),
                    rx.button(
                        "Save Changes",
                        type="submit",
                    ),
                    spacing="3",
                    justify="end",
                ),
                width="100%",
                spacing="4",
            ),
            on_submit=lambda form_data: devices_table_state.update_device(form_data),
            reset_on_submit=True,
        )




@template(route="/devices", title="Devices", on_load=devices_table_state.initialize)
def devices() -> rx.Component:
    return rx.hstack(
        device_section(),
        rx.divider(orientation="vertical", size="4"),
        device_info_card(),
        width="100%",
    )
