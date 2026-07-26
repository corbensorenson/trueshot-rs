from typing import Optional
from ..templates import template
from reflex_ag_grid import ag_grid
from sqlmodel import SQLModel, select
import reflex as rx
from pga import MeshModel
from pga import Job
from pga import Machine
from ..photogrammetry.widgets import *
from sqlalchemy.dialects.postgresql import UUID
from ..components.search_bar import search_bar
from ..components.ag_grid_baseTable import selectable_ag_table
#from .machines_state import machines_table_state

class jobManager_state(rx.State):
    data: list[dict] = []
    selected_job: Job = None
    selected_job_id: str = None
    show_dialog: bool = False
    search_by_term: str = ''
    search_by_options: list[str]= ["name", "id", "created_at", "number"]
    search_by_current: str = "name"
    filter_options:list[str] = ["none", "machine", "model"]
    filter_option_current:str = "none" # "machine", "model" other options
    show_by_options:list[str] = ["not started", "qued", "ongoing", "completed", "all - finished", "all"]
    show_by_current:str = "all - finished"
    current_filter_id:str = ""

    selected_machine_id:str = ""
    selected_machine:Machine=None

    selected_model_id:str = ""
    selected_model:MeshModel = None

    heading_name:str = ""

    @rx.event
    def initialize(self):
        self.search_by_term = ""
        self.search_by_current = "name"
        self.filter_option_current = "none"
        self.show_by_current = "all - finished"
        self.current_filter_id = ""
        self.selected_job = None
        self.selected_job_id = ""
        self.show_dialog = False
        self.heading_name = ""
        self.selected_machine = None
        self.selected_machine_id = ""
        self.selected_model = None
        self.selected_model_id = ""

    @rx.var
    def has_selected_machine(self) -> bool:
        """Check if a device is currently selected."""
        return self.selected_machine is not None

    @rx.event
    def set_search_by_current(self, term:str):
        self.search_by_current = term

    @rx.event
    def set_search_by_term(self, term:str):
        self.search_by_term = term

    @rx.event
    def set_current_show_by(self, term:str):
        self.show_by_current = term
        self.load_data()

    @rx.event
    def load_data(self, filter_option_current="none", filter_id=None):
        """Load initial data from database"""
        self.filter_option_current = filter_option_current
        self.current_filter_id = filter_id

        with rx.session() as session:
            query = select(Job)

            # Apply filter options
            if self.filter_option_current == self.filter_options[1]:
                # Load jobs where the jobs machine_id = filter_id
                query = query.where(Job.machine_id == self.current_filter_id)
            elif self.filter_option_current == self.filter_options[2]:
                # Load jobs where the jobs model_id = filter_id
                pass
                #query = query.where(Job.mesh_model_id == self.current_filter_id)

            # Apply show options 
            if self.show_by_current == self.show_by_options[0]:
                # Filter out all jobs but those that aren't started yet
                query = query.where(Job.status == 0)
            elif self.show_by_current == self.show_by_options[1]:
                # Filter out all jobs but those that are in qued
                query = query.where(Job.status == 1) 
            elif self.show_by_current == self.show_by_options[2]:
                # Filter out all jobs but those that are in progress
                query = query.where(Job.status == 2) 
            elif self.show_by_current == self.show_by_options[3]:
                # Filter out all jobs but those that are finished
                query = query.where(Job.status == 3) 
            elif self.show_by_current == self.show_by_options[4]:
                # Filter out all completed jobs but keep rest
                query = query.where(Job.status != 3) 
            #5 is equal to all so it doesnt need a section for show

            results = session.exec(query).all()
            self.data = [{**result.dict(), 'id': str(result.id)} for result in results]
        

    @rx.event
    def load_from_machine_selection(self, row_data:dict):
        if row_data:
            with rx.session() as session:
                # Query the full model from the database
                machine = session.exec(
                    select(Machine).where(Machine.id == row_data['data']['id'])
                ).first()
                if machine:
                    self.initialize()
                    self.heading_name = machine.name
                    self.selected_machine_id = str(machine.id)
                    self.selected_machine = machine
                    self.load_data("machine", str(machine.id))

    @rx.event
    def load_from_model_selection(self, row_data:dict):
        if row_data:
            with rx.session() as session:
                # Query the full model from the database
                model = session.exec(
                    select(MeshModel).where(MeshModel.id == row_data['data']['id'])
                ).first()
                if model:
                    self.initialize()
                    self.heading_name = model.name
                    self.selected_model_id = str(model.id)
                    self.selected_model = model
                    self.load_data("model", str(model.id))

    @rx.event
    def search_data(self):
        with rx.session() as session:
            try:
                if self.search_by_term:
                    search_column = getattr(Job, self.search_by_current)
                    
                    # Handle different column types
                    if self.search_by_current == "number":
                        # For integer columns, cast the search term to integer
                        try:
                            search_value = int(self.search_by_term)
                            query = select(Job).where(search_column == search_value)
                        except ValueError:
                            # If search term isn't a valid integer, return no results
                            self.data = []
                            return
                    else:
                        # For string columns, use ILIKE
                        query = select(Job).where(
                            search_column.ilike(f"%{self.search_by_term.lower()}%")
                        )
                    
                    results = session.exec(query).all()
                    self.data = [result.dict() for result in results]
                    # Convert UUID to string for display
                    for job in self.data:
                        job['id'] = str(job['id'])
                else:
                    # If no search term, load all data
                    self.load_data()
            except Exception as e:
                return rx.window_alert(f"Search error: {str(e)}")
            
    @rx.event
    def clear_search(self):
        self.search_by_term = ""
        self.load_data()

    @rx.event
    def cell_value_changed(self, row, col_field, new_value):
        """Handle cell value changes"""
        with rx.session() as session:
            model_data = self.data[row]
            model_data[col_field] = new_value
            model = Job(**model_data)
            session.merge(model)
            session.commit()
            self.load_data()
        yield rx.toast(f"Cell value changed, Row: {row}, Column: {col_field}, New Value: {new_value}")

    @rx.event
    def open_dialog(self, row_data: dict):
        if row_data:
            with rx.session() as session:
                # Query the full model from the database
                self.selected_job_id = row_data['data']['id']
                job = session.exec(
                    select(Job).where(Job.id == self.selected_job_id)
                ).first()
                if job:
                    self.selected_job = job
                    self.show_dialog = True    

    @rx.event
    def close_dialog(self):
        """Close the dialog"""
        self.selected_job_id = None
        self.selected_job = None
        self.show_dialog = False

    @rx.event
    def add_new_job(self, form_data: dict):
        """Handle the form submit."""
        job = Job(
            name=form_data["name"],
            processor=form_data["processor"],
            priority=int(form_data["priority"]),
            status=form_data["status"],
            progress=form_data["progress"],
            config=form_data["config"]
        )
        job.save()
        self.load_data()

    @rx.event
    def update_job(self, form_data: dict):
        """Handle the form submit for updating a job."""
        with rx.session() as session:
            job = session.exec(select(Job).where(Job.id == self.selected_job_id)).first()
            if job:
                job.name = form_data["name"]
                job.processor = form_data["processor"]
                job.priority = int(form_data["priority"])
                job.status = form_data["status"]
                job.progress = form_data["progress"]
                job.config = form_data["config"]
                session.add(job)
                session.commit()
                self.load_data()
                self.close_dialog()

def job_search_bar() -> rx.Component:
    return rx.hstack(
        search_bar(jobManager_state),
        rx.select(jobManager_state.show_by_options, value=jobManager_state.show_by_current, on_change=jobManager_state.set_current_show_by)
    ),

def add_job_popup() -> rx.Component:
    """The add job page with a form to add a new job."""
    return rx.dialog.root(
        rx.dialog.trigger(rx.button("New Job", size="3")),
        rx.dialog.content(
            rx.dialog.title("Add a new job"),
            rx.dialog.description("Fill in the job details"),
            rx.form(
                rx.vstack(
                    rx.hstack(
                        rx.text("Name:", size="3"),
                        rx.input(
                            placeholder="Job Name",
                            name="name",
                            required=True
                        ),
                    ),
                    rx.hstack(
                        rx.text("Processor:", size="3"),
                        rx.input(
                            placeholder="Processor Type",
                            name="processor"
                        ),
                    ),
                    rx.hstack(
                        rx.text("Priority:", size="3"),
                        rx.input(
                            type="number",
                            placeholder="0-100",
                            name="priority"
                        ),
                    ),
                    rx.hstack(
                        rx.text("Status:", size="3"),
                        rx.select(
                            ["Pending", "Running", "Complete", "Failed"],
                            placeholder="Select Status",
                            name="status"
                        ),
                    ),
                    rx.hstack(
                        rx.text("Progress:", size="3"),
                        rx.input(
                            placeholder="Progress %",
                            name="progress"
                        ),
                    ),
                    rx.hstack(
                        rx.text("Config:", size="3"),
                        rx.text_area(
                            placeholder="Job Configuration JSON",
                            name="config"
                        ),
                    ),
                    rx.hstack(
                        rx.spacer(),
                        rx.dialog.close(
                            rx.button(
                                "Add Job",
                                type="submit",
                                size="3"
                            )
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
                on_submit=jobManager_state.add_new_job,
                reset_on_submit=True,
            )
        )
    )
    
def view_job_popup() -> rx.Component:
    return rx.dialog.root(
        #rx.cond(
        #    include_button,
        #    rx.dialog.trigger(
        #        rx.button("view job", on_click=state.open_dialog)
        #    ),
        #),
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("Job Information", padding_top="12", size="5"),
                rx.spacer(),
                rx.dialog.close(
                    rx.button("X", size="2", border="0", on_click=jobManager_state.close_dialog),
                ),
            ),
            rx.vstack(
                rx.hstack(
                    rx.text("Job Name: "),
                    rx.text(jobManager_state.selected_job.name),
                ),
                rx.hstack(
                    rx.text("Processor: "),
                    rx.text(jobManager_state.selected_job.processor),
                ),
                rx.hstack(
                    rx.text("Priority: "),
                    rx.text(jobManager_state.selected_job.priority),
                ),
                rx.hstack(
                    rx.text("Status: "),
                    rx.text(jobManager_state.selected_job.status),
                ),
                rx.hstack(
                    rx.text("Progress: "),
                    rx.text(jobManager_state.selected_job.progress),
                ),
                rx.hstack(
                    rx.text("Start Time: "),
                    rx.text(jobManager_state.selected_job.start_time),
                ),
                rx.hstack(
                    rx.text("End Time: "),
                    rx.text(jobManager_state.selected_job.end_time),
                ),
                rx.divider(),
                rx.text("Configuration:", size="4"),
                rx.code_block(
                    jobManager_state.selected_job.config,
                    language="json",
                    show_line_numbers=True,
                ),
                width="100%",
                spacing="3",
            ),
            max_width="600px",
        ),
        open=jobManager_state.show_dialog,
    )
    
    
def edit_job_popup() -> rx.Component:
    """The edit job page with a form to edit an existing job."""
    return rx.dialog.root(
        rx.dialog.trigger(rx.button("Edit Job", size="3")),
        rx.dialog.content(
            rx.dialog.title("Edit Job"),
            rx.dialog.description("Edit the job details"),
            rx.form(
                rx.vstack(
                    rx.hstack(
                        rx.text("Name:", size="3"),
                        rx.input(
                            value=jobManager_state.selected_job.name,
                            name="name",
                            required=True
                        ),
                    ),
                    rx.hstack(
                        rx.text("Processor:", size="3"),
                        rx.input(
                            value=jobManager_state.selected_job.processor,
                            name="processor"
                        ),
                    ),
                    rx.hstack(
                        rx.text("Priority:", size="3"),
                        rx.input(
                            type="number",
                            value=jobManager_state.selected_job.priority,
                            name="priority"
                        ),
                    ),
                    rx.hstack(
                        rx.text("Status:", size="3"),
                        rx.input(
                            type="number",
                            value=jobManager_state.selected_job.status,
                            name="status"
                        ),

                    ),
                    rx.hstack(
                        rx.text("Progress:", size="3"),
                        rx.input(
                            value=jobManager_state.selected_job.progress,
                            name="progress"
                        ),
                    ),
                    rx.hstack(
                        rx.text("Config:", size="3"),
                        rx.text_area(
                            value=jobManager_state.selected_job.config,
                            name="config"
                        ),
                    ),
                    rx.hstack(
                        rx.spacer(),
                        rx.dialog.close(
                            rx.button(
                                "Save Changes",
                                type="submit",
                                size="3"
                            )
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
                on_submit=jobManager_state.update_job,
                reset_on_submit=True,
            )
        ),
        open=jobManager_state.show_dialog,
    )

def jobs_table(col_defs = None) -> rx.Component:
    if col_defs == None:
        col_defs = [
            ag_grid.column_def(field="name", header_name="Job Name", editable=True),
            ag_grid.column_def(field="processor", header_name="Processor", editable=True),
            ag_grid.column_def(field="priority", header_name="Priority", editable=True),
            ag_grid.column_def(field="status", header_name="Status", editable=True),
            #ag_grid.column_def(field="progress", header_name="Progress", editable=True),
            #ag_grid.column_def(field="start_time", header_name="Start Time", editable=False),
            #ag_grid.column_def(field="end_time", header_name="End Time", editable=False),
            #ag_grid.column_def(field="config", header_name="Config", editable=True),
        ]
    return selectable_ag_table("jobs_table", col_defs, jobManager_state)


def jobs_section() -> rx.Component:
    return rx.card(
            rx.vstack(
                rx.center(
                    rx.spacer(),
                    rx.icon("briefcase"),
                    rx.heading(f"{jobManager_state.heading_name} jobs", size="5", margin_left="5px"),
                    rx.spacer(),
                    add_job_popup(),
                    width="100%",
                ),
                job_search_bar(),
                jobs_table(),
            ),
            width="100%",
        ),









""" 
def job_popup(state: jobManager_state) -> rx.Component:

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
                rx.foreach(jobManager_state.device_config_data, show_data)
            ),
            width="100%",
        )

    return rx.dialog.root(
        rx.dialog.content(
            rx.hstack(
                rx.dialog.title("job Information", margin_top="7", size="5"),
                rx.spacer(),
                rx.dialog.close(
                    rx.button("X", size="2", border="0", on_click=state.close_dialog),
                ),
            ),
            rx.hstack(
                rx.text("Device Name: "),
                rx.text(state.selected_device.name),
            ),
            rx.hstack(
                rx.text("Device Description: "),
                rx.text(state.selected_device.description),
            ),
            rx.hstack(
                rx.text("Device Category: "),
                rx.text(state.selected_device.category),
            ),
            rx.hstack(
                rx.text("Device Implementation: "),
                rx.text(state.selected_device.implementation),
            ),
            rx.hstack(
                rx.text("additional information: "),
                rx.text(state.selected_device.notes),
            ),
            rx.hstack(
                rx.text("Device Config: "),
                data_table(),
            ),
        ),
        open=state.show_dialog,
    )



 """


