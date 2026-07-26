import reflex as rx
from reflex_ag_grid import ag_grid


def selectable_ag_table(id:str, col_defs, state, on_mount = True) -> rx.Component:
    if on_mount:
        return ag_grid(
            id=id,
            row_data=state.data,
            column_defs=col_defs,
            on_cell_value_changed=state.cell_value_changed,
            on_mount=state.load_data,
            row_selection="single",  # Enable single row selection
            on_row_selected=state.open_dialog,  # Handle row selection
            width="100%",
            height="40vh",
        )
    else:
        return ag_grid(
            id=id,
            row_data=state.data,
            column_defs=col_defs,
            on_cell_value_changed=state.cell_value_changed,
            row_selection="single",  # Enable single row selection
            on_row_selected=state.open_dialog,  # Handle row selection
            width="100%",
            height="40vh",
        )