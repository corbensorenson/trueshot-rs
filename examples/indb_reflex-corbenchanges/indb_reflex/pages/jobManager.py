from ..templates import template
from ..states.jobManager_state import *
from ..components.ag_grid_baseTable import selectable_ag_table
from ..components.search_bar import search_bar
from reflex_ag_grid import ag_grid

import reflex as rx


@template(route="/jobManager", title="Job Manager", on_load=jobManager_state.initialize)
def jobManager() -> rx.Component:
    """The job manager page. This page will allow the user to manage the jobs for the project.

    Returns:
        The UI for the job manager page.
    """
    return jobs_section()