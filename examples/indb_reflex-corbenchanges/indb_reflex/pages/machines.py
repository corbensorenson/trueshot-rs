import sqlalchemy
from ..templates import template
import reflex as rx
from reflex_ag_grid import ag_grid
from sqlmodel import SQLModel, select
from pga import Machine
from pga import Device
from pga import Job
from ..states.machines_state import machines_table_state
from ..states.jobManager_state import jobManager_state, jobs_section
from ..components.search_bar import search_bar
from ..components.ag_grid_baseTable import selectable_ag_table




def machine_search_bar() -> rx.Component:
    return search_bar(machines_table_state)


def machines_table() -> rx.Component:
    col_defs = [
        ag_grid.column_def(field="name", header_name="Machine Name", editable=True),
        ag_grid.column_def(field="connected", header_name="Connected", editable=True),
        ag_grid.column_def(field="status", header_name="Status", editable=True),
    ]
    return ag_grid(
            id="machines table",
            row_data=machines_table_state.data,
            column_defs=col_defs,
            on_cell_value_changed=machines_table_state.cell_value_changed,
            on_mount=machines_table_state.load_data,
            row_selection="single",  # Enable single row selection
            on_row_selected=jobManager_state.load_from_machine_selection,  # Handle row selection
            width="100%",
            height="40vh",
        )


def machine_section() -> rx.Component:
    return rx.card(
            rx.vstack(
                rx.center(
                    rx.cond(
                        jobManager_state.has_selected_machine,
                        rx.flex(
                            rx.flex(
                                rx.tooltip(
                                    rx.button(rx.icon("pencil"), on_click=machines_table_state.open_edit_dialog),
                                    content=f"Edit {jobManager_state.heading_name}"
                                ),
                            ),
                            rx.flex(
                                rx.tooltip(
                                    rx.button(rx.icon("layers-2"), on_click=machines_table_state.copy_machine(jobManager_state.selected_machine_id)),
                                    content=f"Create copy of {jobManager_state.heading_name}"
                                ),
                                padding_left="9px",
                            ),
                            rx.flex(
                                rx.tooltip(
                                    rx.flex(
                                        delete_machine_popup_button(),
                                    ),
                                    content=f"Delete {jobManager_state.heading_name}"
                                ),
                                padding_left="9px",
                            ),
                        ),
                    ),
                    rx.spacer(),
                    rx.icon("cpu"),
                    rx.heading("Machines", size="5", margin_left="5px"),
                    rx.spacer(),
                    
                    addMachine(),
                    width="100%",
                ),
                machine_search_bar(),
                machines_table(),
                edit_machine_popup(),
            ),
            max_width="50vw",
        ),

def delete_machine_popup_button() -> rx.Component:
    return rx.dialog.root(
        rx.dialog.trigger(
            rx.button(rx.icon("trash"),)
        ),
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("Delete Machine", padding_top="12", size="5"),
                rx.spacer(),
                rx.dialog.close(
                    rx.button("X", size="2", border="0"),
                ),
            ),
            rx.hstack(
                rx.text("Machine Name: "),
                rx.text(jobManager_state.selected_machine.name),
            ),
            rx.hstack(
                rx.text("Machine Description: "),
                rx.text(jobManager_state.selected_machine.description),
            ),
            rx.hstack(
                rx.text("Confirm Delete: "),
                rx.dialog.close(
                    rx.button("Cancel"),
                    rx.button("Delete", on_click=machines_table_state.delete_machine(jobManager_state.selected_machine_id)),
                ),
            ),
            max_width="450px",
        ),
    )

def edit_machine_popup() -> rx.Component:
    return rx.dialog.root(
        rx.dialog.content(
            rx.dialog.title(
                "Edit Machine",
            ),
            rx.dialog.description(
                "Update machine details below",
            ),
            rx.form(
                rx.flex(
                    rx.hstack(
                        rx.heading("Name: ", size="3"),
                        rx.input(
                            placeholder="Name", 
                            name="name",
                            default_value=jobManager_state.selected_machine.name,
                            required=True
                        ),
                        align="center",
                        width="100%"
                    ),
                    rx.hstack(
                        rx.heading("Description: ", size="3"),
                        rx.input(
                            placeholder="Description",
                            name="description",
                            default_value=jobManager_state.selected_machine.description,
                        ),
                        align="center",
                        width="100%"
                    ),
                    rx.hstack(
                        rx.heading("OS: ", size="3"),
                        rx.input(
                            placeholder="Operating System",
                            name="os",
                            default_value=jobManager_state.selected_machine.os
                        ),
                        align="center",
                        width="100%"
                    ),
                    rx.hstack(
                        rx.heading("CPU: ", size="3"),
                        rx.input(
                            placeholder="CPU",
                            name="cpu",
                            default_value=jobManager_state.selected_machine.cpu
                        ),
                        align="center",
                        width="100%"
                    ),
                    rx.hstack(
                        rx.heading("GPU: ", size="3"),
                        rx.input(
                            placeholder="GPU",
                            name="gpu",
                            default_value=jobManager_state.selected_machine.gpu
                        ),
                        align="center",
                        width="100%"
                    ),
                    rx.hstack(
                        rx.heading("RAM: ", size="3"),
                        rx.input(
                            placeholder="RAM",
                            name="ram",
                            type="number",
                            value=jobManager_state.selected_machine.ram
                        ),
                        align="center",
                        width="100%"
                    ),
                    rx.flex(
                        rx.dialog.close(
                            rx.button(
                                "Cancel",
                                variant="soft",
                                color_scheme="gray",
                                on_click=machines_table_state.close_edit_dialog
                            ),
                            rx.button(
                                "Submit", 
                                type="submit"
                            ),
                        ),
                        spacing="3",
                        justify="end",
                    ),
                    direction="column",
                    spacing="4",
                ),
                on_submit=lambda x:machines_table_state.update_machine(x, jobManager_state.selected_machine_id),
                reset_on_submit=False,
            ),


            max_width="450px",
        ),
        open=machines_table_state.show_edit_dialog,
    )

def addMachine() -> rx.Component:
    """The add machine page with a form to add a new machine."""
    return rx.dialog.root(
        rx.dialog.trigger(rx.tooltip(rx.button(rx.icon("square-plus")), content="Add Machine")),
        rx.dialog.content(
            rx.dialog.title("Add a new machine"),
            rx.dialog.description("Fill in the machine details"),
            rx.form.root(
                rx.vstack(
                    rx.form.field(
                        rx.hstack(
                            rx.text("Name:", size="3"),
                            rx.form.control(
                                rx.input(placeholder="Machine Name"),
                                as_child=True
                            ),
                        ),
                        name="name",
                    ),
                    rx.form.field(
                        rx.hstack(
                            rx.text("Description:", size="3"),
                            rx.form.control(
                                rx.input(placeholder="Machine Description"),
                                as_child=True
                            ),
                        ),
                        name="description", 
                    ),
                    rx.form.field(
                        rx.hstack(
                            rx.text("OS:", size="3"),
                            rx.form.control(
                                rx.input(placeholder="Machine OS"),
                                as_child=True
                            ),
                        ),
                        name="os",
                    ),
                    rx.form.field(
                        rx.hstack(
                            rx.text("CPU:", size="3"), 
                            rx.form.control(
                                rx.input(placeholder="Machine CPU"),
                                as_child=True
                            ),
                        ),
                        name="cpu",
                    ),
                    rx.form.field(
                        rx.hstack(
                            rx.text("GPU:", size="3"),
                            rx.form.control(
                                rx.input(placeholder="Machine GPU"),
                                as_child=True
                            ),
                        ),
                        name="gpu",
                    ),
                    rx.form.field(
                        rx.hstack(
                            rx.text("RAM:", size="3"),
                            rx.form.control(
                                rx.input(placeholder="Machine RAM"),
                                as_child=True
                            ),
                        ),
                        name="ram",
                    ),
                    rx.hstack(
                        rx.spacer(),
                        rx.form.submit(
                            rx.dialog.close(
                                rx.button("Add Machine", size="3")
                            ),
                            as_child=True
                        ),
                        rx.spacer(),
                        rx.dialog.close(
                            rx.button("Cancel", size="3")
                        ),
                        rx.spacer(),
                    ),
                    width="100%",
                    spacing="3",
                ),
                on_submit=machines_table_state.add_new_machine,
                reset_on_submit=True,
            )
        )
    )

@template(route="/machines", title="Machines", on_load=machines_table_state.initialize)
def machines() -> rx.Component:
    """The machines and devices page."""
    return rx.hstack(
        machine_section(),
        rx.flex(
            rx.divider(orientation="vertical", size="4"),
            direction="column",
            width="3",
            height="100%",
        ),
        rx.cond(
            jobManager_state.has_selected_machine,
            rx.flex(
                jobs_section(),
            ),
            rx.card(
                rx.hstack(
                    rx.spacer(),
                    rx.icon("shield-alert"),
                    rx.heading("Select a Machine to see it's Jobs", size="4"),
                    rx.spacer(),
                    width="100%",
                ),
                width="100%",
                max_width="50vw",
                align="center",
            ),
        ),
        width="100%",
    )
