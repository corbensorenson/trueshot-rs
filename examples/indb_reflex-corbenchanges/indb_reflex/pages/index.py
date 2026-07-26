"""The overview page of the app."""

import reflex as rx
from .. import styles
from ..templates import template
from ..views.stats_cards import stats_cards
from ..views.charts import (
    users_chart,
    revenue_chart,
    orders_chart,
    area_toggle,
    pie_chart,
    timeframe_select,
    StatsState,
)
from ..views.adquisition_view import adquisition
from ..components.notification import notification
from ..components.card import card
from .profile import ProfileState
import datetime


def _time_data() -> rx.Component:
    return rx.hstack(
        rx.tooltip(
            rx.icon("info", size=20),
            content=f"{(datetime.datetime.now() - datetime.timedelta(days=30)).strftime('%b %d, %Y')} - {datetime.datetime.now().strftime('%b %d, %Y')}",
        ),
        rx.text("Last 30 days", size="4", weight="medium"),
        align="center",
        spacing="2",
        display=["none", "none", "flex"],
    )


def tab_content_header() -> rx.Component:
    return rx.hstack(
        _time_data(),
        area_toggle(),
        align="center",
        width="100%",
        spacing="4",
    )

def section(SectionName, sectionTitle, onChange = None) -> rx.Component:
    return rx.vstack(
        rx.text(sectionTitle, size="4", weight="medium"),
        rx.hstack(
            rx.text("Implementation", size="3", weight="bold"),
            rx.select(
                #should query these
                ["Photo Capture", "Processing", "Modeling", "Export"],
                value=f"{SectionName}_implementation",
                width="200px",
                #on_change=onChange,
            ),
            rx.text("Machine", size="3", weight="bold"),
            rx.select(
                #should query these
                ["Comp 1", "Comp 2", "Comp 3"],
                value=f"{SectionName}_machine",
                width="200px",
                #on_change=onChange,
            ),
            rx.text("Input:", size="3", weight="bold"),
            rx.input(name = f"{SectionName}_input", placeholder="/input_location", width="200px", debounce_timeout=500),
            rx.text("Output:", size="3", weight="bold"),
            rx.input(name = f"{SectionName}_output", placeholder="/output_location", width="200px", debounce_timeout=500),
        ),
    )

@template(route="/", title="Pipeline", on_load=StatsState.randomize_data)
def index() -> rx.Component:
    """The overview page.

    Returns:
        The UI for the overview page.
    """
    return rx.vstack(
        rx.heading(f"The pipeline", size="5"),
        rx.divider(),
        section("cam", "Camera Capture"),
        rx.divider(),
        section("hdr", "HDR combination"),
        rx.divider(),
        section("ps", "Photo Stacking"),
        rx.divider(),
        section("bgr", "Background Removal"),
        rx.divider(),
        section("pg", "Photogrammetry"),
        rx.divider(),
        section("mc", "Model Cleanup"),
        spacing="6",
        width="100%",
    )
