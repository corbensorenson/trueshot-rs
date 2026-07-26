import reflex as rx
from ..states.photoAcquisition_state import AcquisitionsState, Item
from ..components.status_badge import status_badge


def _header_cell(text: str) -> rx.Component:
    return rx.table.column_header_cell(
        rx.hstack(
            rx.text(text),
            align="center",
            spacing="2",
        ),
    )


def _show_item(item: Item, index: int) -> rx.Component:
    bg_color = rx.cond(
        index % 2 == 0,
        rx.color("gray", 1),
        rx.color("accent", 2),
    )
    hover_color = rx.cond(
        index % 2 == 0,
        rx.color("gray", 3),
        rx.color("accent", 3),
    )
    return rx.table.row(
        # rx.table.row_header_cell(item.id),
        rx.table.cell(item.n),
        rx.table.cell(item.orientation),
        rx.table.cell(item.description),
        style={"_hover": {"bg": hover_color}, "bg": bg_color},
        align="center",
    )

def acquisitions_table() -> rx.Component:
    return rx.box(
        rx.table.root(
            rx.table.header(
                rx.table.row(
                    _header_cell("N"),
                    _header_cell("Orientation"),
                    _header_cell("Description"),
                ),
            ),
            rx.table.body(
                rx.foreach(
                    AcquisitionsState.get_items,
                    lambda item, index: _show_item(item, index),
                )
            ),
            variant="surface",
            size="3",
            width="100%",
        ),
        width="100%",
    )
