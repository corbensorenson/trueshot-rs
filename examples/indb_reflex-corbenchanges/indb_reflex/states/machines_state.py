import sqlalchemy
import reflex as rx
from reflex_ag_grid import ag_grid
from sqlmodel import SQLModel, select
from pga import Machine
from pga import Device
from .jobManager_state import jobManager_state

class machines_table_state(rx.State):
    data: list[dict] = [] # Change models to data to match AG Grid expectations
    selected_machine: Machine = None
    selected_machine_name:str = ""
    show_dialog: bool = False
    show_edit_dialog: bool = False
    search_by_term: str = ''
    search_by_options: list[str]= ["name", "id", "created_at"]
    search_by_current: str = "name"

    @rx.event
    def initialize(self):
        yield jobManager_state.initialize()
        self.search_by_term = ""
        self.search_by_current = "name"
        self.selected_machine = None
        self.show_dialog = False
        self.show_edit_dialog = False
        self.load_data()

    @rx.var
    def has_selected_machine(self) -> bool:
        """Check if a device is currently selected."""
        return self.selected_machine is not None

    @rx.event
    def open_edit_dialog(self):
        print("opening edit dialog")
        print(self.show_edit_dialog)
        self.show_edit_dialog = True

    @rx.event
    def close_edit_dialog(self):
        print("closing edit dialog")
        print(self.show_edit_dialog)
        self.show_edit_dialog = False

    @rx.event
    def set_search_by_current(self, term:str):
        self.search_by_current = term

    @rx.event
    def set_search_by_term(self, term:str):
        self.search_by_term = term

    @rx.event
    def load_data(self):
        """Load initial data from database"""
        self.data = [{**result.dict(), 'id': str(result.id)} for result in Machine.all()] 

    @rx.event
    def search_data(self):
        with rx.session() as session:
            try:
                if self.search_by_term:
                    search_column = getattr(Machine, self.search_by_current)
                    # For string columns, use ILIKE
                    query = select(Machine).where(
                        search_column.ilike(f"%{self.search_by_term.lower()}%")
                    )
                    results = session.exec(query).all()
                    self.data = [result.dict() for result in results]
                    # Convert UUID to string for display
                    for machine in self.data:
                        machine['id'] = str(machine['id'])
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
            model = Machine(**model_data)
            session.merge(model)
            session.commit()
            self.load_data()
        yield rx.toast(f"Cell value changed, Row: {row}, Column: {col_field}, New Value: {new_value}")

    @rx.event
    def open_dialog(self, row_data: dict):
        if row_data:
            with rx.session() as session:
                # Query the full model from the database
                machine = session.exec(
                    select(Machine).where(Machine.id == row_data['data']['id'])
                ).first()
                if machine:
                    self.selected_machine = machine
                    self.selected_machine_name = str(machine.name)
                    print(self.selected_machine_name)


    @rx.event
    def close_dialog(self):
        """Close the dialog"""
        self.selected_machine = None
        self.show_dialog = False

    
    @rx.event
    def add_new_machine(self, form_data:dict):
        m = Machine(name= form_data["name"], os = form_data["os"], cpu = form_data["cpu"], 
                    gpu=form_data["gpu"], ram = form_data["ram"], description=form_data["description"])
        m.save()
        self.initialize()
        self.load_data()

    @rx.event
    def update_machine(self, form_data: dict, id):
        # Update the machine with form_data
        m = Machine(id =id ,name= form_data["name"], os = form_data["os"], cpu = form_data["cpu"], 
                    gpu=form_data["gpu"], ram = form_data["ram"], description=form_data["description"])
        m.save()
        self.initialize()
        self.load_data()
        self.close_edit_dialog()

    @rx.event
    def copy_machine(self, id):
        m = Machine(id=id)
        m.copy(id)
        self.initialize()
        self.load_data()

    @rx.event
    def delete_machine(self, id):
        m = Machine(id=id)
        m.delete(id)
        self.initialize()
        self.load_data()


