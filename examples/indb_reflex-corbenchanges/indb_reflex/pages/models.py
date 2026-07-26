from ..templates import template
from reflex_ag_grid import ag_grid
import reflex as rx

from pga import CameraPreset
from ..photogrammetry.widgets import *
from ..states.models_state import models_state, associated_sequences_table_state
from ..components.search_bar import search_bar
from ..components.ag_grid_baseTable import selectable_ag_table
from ..states.jobManager_state import *


class models_table_state(models_state):
    @rx.event
    def on_row_select(self, row_data: dict):
        yield models_state.open_dialog(row_data)
        yield jobManager_state.load_from_model_selection(row_data)



def model_search_bar() -> rx.Component:
    return search_bar(models_state)

def associated_sequences_table() -> rx.Component:
    col_defs = [
        ag_grid.column_def(field="sequence_number", header_name="Sequence Number", editable=True, cell_editor=ag_grid.editors.number,),
        ag_grid.column_def(field="description", header_name="Description", editable=True, cell_editor=ag_grid.editors.text,),
        ag_grid.column_def(field="orientation", header_name="orientation", editable=True, cell_editor=ag_grid.editors.text,),
    ]
    return selectable_ag_table("associated_sequences_table", col_defs, associated_sequences_table_state, False)


def models_table() -> rx.Component:
    col_defs = [
        ag_grid.column_def(field="name", header_name="Model Name", editable=True),
        ag_grid.column_def(field="number", header_name="Model Number", editable=False),
        ag_grid.column_def(field="description", header_name="Description", editable=True),
        ag_grid.column_def(field="notes", header_name="Notes", editable=True),
        ag_grid.column_def(field="created_at", header_name="Created", editable=False),
        ag_grid.column_def(field="id", header_name="id", editable=False),
    ]
    return ag_grid(
            id="models table",
            row_data=models_state.data,
            column_defs=col_defs,
            on_cell_value_changed=models_state.cell_value_changed,
            on_mount=models_state.load_data,
            row_selection="single",  # Enable single row selection
            on_row_selected=models_table_state.on_row_select,  # Handle row selection
            width="100%",
            height="40vh",
        )

def delete_model_popup_button() -> rx.Component:
    return rx.dialog.root(
        rx.dialog.trigger(
            rx.button(rx.icon("trash"), size="2")
        ),
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("Delete Model", padding_top="12", size="5"),
                rx.spacer(),
                rx.dialog.close(
                    rx.button("X", size="2", border="0"),
                ),
            ),
            rx.hstack(
                rx.text("Model Name: "),
                rx.text(models_state.selected_model.name),
            ),
            rx.hstack(
                rx.text("Model Description: "),
                rx.text(models_state.selected_model.description),
            ),
            rx.hstack(
                rx.text("Confirm Delete: "),
                rx.dialog.close(
                    rx.button("Cancel"),
                    rx.button("Delete", on_click=models_state.delete_model(models_state.selected_model_id)),
                ),
            ),
            max_width="450px",
        ),
    )


def model_view_area() -> rx.Component:
    return rx.card(
        rx.cond(
            ~models_state.has_selected_model,
            rx.hstack(
                rx.spacer(),
                rx.icon("shield-alert"),
                rx.heading("No Model Selected", size="4"),
                rx.spacer(),
                width="100%",
            ),
            rx.vstack(
                rx.hstack(
                    rx.spacer(),
                    rx.icon("box"),
                    rx.heading(f"Model info on {models_state.selected_model.name}", padding_top="12", size="6"),
                    rx.spacer(),
                    width="100%",
                ),
                rx.hstack(
                    rx.link(rx.button("Open in 3d"), href=f'https://ineurodb.org/desktop/scenetest.html', is_external=True),#'/modelViewer?id={models_state.selected_model_id}'
                    rx.link(rx.button("Open in Photo Acquisition"), href=f'/photoAcquisition?id={models_state.selected_model_id}'), 
                    rx.spacer(),
                    rx.tooltip(rx.flex(delete_model_popup_button()), content=f"Delete {models_state.selected_model.name}"),
                    rx.cond(
                        models_state.editable,
                        rx.tooltip(rx.flex(rx.button(rx.icon("ban"), on_click=models_state.toggle_editable),), content=f"Cancel Editing {models_state.selected_model.name}"),
                        rx.tooltip(rx.flex(rx.button(rx.icon("pencil"), on_click=models_state.toggle_editable),), content=f"Edit {models_state.selected_model.name}"),
                    ),
                    width="100%",
                ),
                rx.image(src=models_state.thumbnail_location_string, width="100%", height="auto", flex_grow="1", flex_shrink="1"),
                rx.hstack(
                    rx.text("Model Name: "),
                    rx.text(models_state.selected_model.name),
                ),
                rx.hstack(
                    rx.text("Model UUID: "),
                    rx.text(models_state.selected_model_id),
                ),
                rx.hstack(
                    rx.text("Date created: "),
                    rx.text(models_state.selected_model.created_at),
                ),
                rx.hstack(
                    rx.text("Model Number: "),
                    rx.text(models_state.selected_model.number),
                ),
                rx.hstack(
                    rx.text("Model Description: "),
                    rx.text(models_state.selected_model.description),
                ),
                rx.hstack(
                    rx.text("additional information: "),
                    rx.text(models_state.selected_model.notes),
                ),
                rx.hstack(
                    rx.spacer(),
                    rx.text("Associated Sequences:", size="4"),
                    rx.spacer(),
                ),
                associated_sequences_table(),
                width="100%",
                
            ),
        ),
        width="100%",
        style={"max-width": "39vw"},
    ),

def camera_preset_popup(camera_preset:CameraPreset) -> rx.Component:

    return rx.dialog.root(
        rx.dialog.trigger(rx.button("View Camera preset")),
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("Camera preset Information", padding_top="12", size="5"),
                rx.spacer(),
                rx.dialog.close(
                    rx.button("X", size="2", border="0"),
                ),
            ),
            rx.hstack(
                rx.text("camera_preset Name: "),
                rx.text(camera_preset.name),
            ),
            rx.hstack(
                rx.text("iso: "),
                rx.text(camera_preset.iso),
            ),
            rx.hstack(
                rx.text("aperture: "),
                rx.text(camera_preset.aperture),
            ),
            rx.hstack(
                rx.text("shutter_speed: "),
                rx.text(camera_preset.shutter_speed),
            ),
            rx.hstack(
                rx.text("exposure_mode: "),
                rx.text(camera_preset.exposure_mode),
            ),
            rx.hstack(
                rx.text("exposure_compensation: "),
                rx.text(camera_preset.exposure_compensation),
            ),
            rx.hstack(
                rx.text("white_balance: "),
                rx.text(camera_preset.white_balance),
            ),
        ),
    )

def sequence_popup(state: associated_sequences_table_state) -> rx.Component:
    return rx.dialog.root(
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("Sequence Information", padding_top="12", size="5"),
                rx.spacer(),
                camera_preset_popup(state.selected_camera_preset),
                rx.spacer(),
                rx.dialog.close(
                    rx.button("X", size="2", border="0", on_click=state.close_dialog),
                ),
            ),
            rx.hstack(
                rx.text("sequence_number: "),
                rx.text(state.selected_sequence.sequence_number),
            ),
            rx.hstack(
                rx.text("orientation: "),
                rx.text(state.selected_sequence.orientation),
            ),
            rx.hstack(
                rx.text("description: "),
                rx.text(state.selected_sequence.description),
            ),
            rx.hstack(
                rx.text("camera_name: "),
                rx.text(state.selected_sequence.camera_name),
            ),
            rx.hstack(
                rx.text("aperture: "),
                rx.text(state.selected_sequence.aperture),
            ),
            rx.hstack(
                rx.text("iso: "),
                rx.text(state.selected_sequence.iso),
            ),
            rx.hstack(
                rx.text("shutter_speed: "),
                rx.text(state.selected_sequence.shutter_speed),
            ),
            rx.hstack(
                rx.text("exposure_mode: "),
                rx.text(state.selected_sequence.exposure_mode),
            ),
            rx.hstack(
                rx.text("rotation_total: "),
                rx.text(state.selected_sequence.rotation_total),
            ),
            rx.hstack(
                rx.text("rotation_step: "),
                rx.text(state.selected_sequence.rotation_step),
            ),
            rx.hstack(
                rx.text("hdr: "),
                rx.text(state.selected_sequence.hdr),
            ),
            rx.cond(
                state.selected_sequence.hdr,
                rx.vstack(
                    rx.hstack(
                        rx.text("hdr_exposures: "),
                        rx.text(state.selected_sequence.hdr_exposures),
                    ),
                    rx.hstack(
                        rx.text("hdr_starting_shutter_speed: "),
                        rx.text(state.selected_sequence.hdr_starting_shutter_speed),
                    ),
                    rx.hstack(
                        rx.text("hdr_step_size: "),
                        rx.text(state.selected_sequence.hdr_step_size),
                    ),
                ),
            ),
            rx.hstack(
                rx.text("focus_stacking: "),
                rx.text(state.selected_sequence.focus_stacking),
            ),
            rx.cond(
                state.selected_sequence.focus_stacking,
                rx.hstack(
                    rx.text("focus_steps: "),
                    rx.text(state.selected_sequence.focus_steps),
                    rx.text("focus_step_width: "),
                    rx.text(state.selected_sequence.focus_step_width),
                ),
            ),
            

        ),
        open=state.show_dialog,
    )

def model_section()->rx.Component:
    return rx.card(
        rx.vstack(
            rx.hstack(
                rx.spacer(),
                rx.icon("box"),
                rx.heading("Select Model"),
                rx.spacer(),
                width="100%",
                align="center"
            ),
            model_search_bar(),
            models_table(),
            sequence_popup(associated_sequences_table_state),
            #spacing="8",
            width="100%",
        ),
        width="100%"
    ),



@template(route="/models", title="Models", on_load=[models_state.initialize, jobManager_state.initialize])
def models() -> rx.Component:
    return rx.hstack(
        rx.spacer(),
        rx.vstack(
            model_section(),
            rx.divider(size="4"),
            jobs_section(),
        ),
        rx.flex(
            rx.divider(orientation="vertical", size="4"),
            direction="column",
            width="3",
            height="100%",
        ),
        model_view_area(),
        rx.spacer(),
        width = "100%",
        style={"max-width": "100vw"},
    )





