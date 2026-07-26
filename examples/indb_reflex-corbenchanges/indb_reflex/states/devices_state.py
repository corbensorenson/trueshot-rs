import sqlalchemy
import reflex as rx
from reflex_ag_grid import ag_grid
from sqlmodel import SQLModel, select
from pga import Machine
from pga import Device
import json

class devices_table_state(rx.State):
    data: list[dict] = [] # Change models to data to match AG Grid expectations
    selected_device: Device = None
    selected_device_id: str = None
    show_dialog: bool = False
    search_by_term: str = ''
    search_by_options: list[str]= ["name", "id", "created_at"]
    search_by_current: str = "name"
    device_config_data: dict = {}
    config_data_string:str = ""

    device_categories:list[str] = ["camera", "turntable", "cardReader", "arm", "all"]
    current_category:str = "all"

    editable:bool = False


    @rx.event
    def toggle_editable(self):
        self.editable = not self.editable

    @rx.event
    def set_current_category(self, category):
        self.current_category = category
        self.load_data()

    @rx.event
    def set_search_by_current(self, term:str):
        self.search_by_current = term

    @rx.event
    def set_search_by_term(self, term:str):
        self.search_by_term = term

    @rx.event
    def initialize(self):
        self.selected_device = None
        self.load_data()
        self.search_by_term = ""
        self.search_by_current = "name"
        self.current_category = "all"
        self.show_dialog = False
        self.device_config_data = {}
        self.config_data_string = ""
        self.editable = False

    @rx.event
    def load_data(self):
        """Load initial data from database"""
        #self.data = [{**result.dict(), 'id': str(result.id)} for result in Device.all()]
        with rx.session() as session:
            query = select(Device)
            if self.current_category != "all":
                # Filter out all jobs but those that aren't started yet
                query = query.where(Device.category == self.current_category)
            results = session.exec(query).all()
            self.data = [{**result.dict(), 'id': str(result.id)} for result in results]

    @rx.event
    def search_data(self):
        with rx.session() as session:
            try:
                if self.search_by_term:
                    search_column = getattr(Device, self.search_by_current)
                    # For string columns, use ILIKE
                    query = select(Device).where(
                        search_column.ilike(f"%{self.search_by_term.lower()}%")
                    )
                    results = session.exec(query).all()
                    self.data = [result.dict() for result in results]
                    # Convert UUID to string for display
                    for device in self.data:
                        device['id'] = str(device['id'])
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
            model = Device(**model_data)
            session.merge(model)
            session.commit()
            self.load_data()
        yield rx.toast(f"Cell value changed, Row: {row}, Column: {col_field}, New Value: {new_value}")

    @rx.event
    def open_dialog(self, row_data: dict):
        if row_data:
            with rx.session() as session:
                # Query the full model from the database
                device = session.exec(
                    select(Device).where(Device.id == row_data['data']['id'])
                ).first()
                if device:
                    self.selected_device = device
                    self.selected_device_id = str(device.id)
                    self.device_config_data = device.config
                    self.config_data_string = str(device.config)
                    self.show_dialog = True    

    @rx.event
    def close_dialog(self):
        """Close the dialog"""
        self.selected_device = None
        self.show_dialog = False

    @rx.event
    def add_new_device(self, form_data:dict):
        d = Device(name=form_data["name"], category=form_data["category"], 
                   implementation = form_data["implementation"], description = form_data["description"],
                   config = "", notes = form_data["notes"], machine_id = "")
        d.save()
        self.load_data()

    @rx.var
    def has_selected_device(self) -> bool:
        """Check if a device is currently selected."""
        return self.selected_device is not None

    @rx.event
    def update_device(self, form_data: dict):
        """Update the selected device with new data."""
        if not self.selected_device:
            return
            
        with rx.session() as session:
            device = session.exec(
                select(Device).where(Device.id == self.selected_device.id)
            ).first()
            
            if device:
                device.name = form_data["name"]
                device.category = form_data["category"]
                device.implementation = form_data["implementation"]
                device.description = form_data["description"]
                device.notes = form_data["notes"]
                device.config = json.loads(form_data["config"])
                
                session.add(device)
                session.commit()
                
                # Refresh the selected device and table data
                self.selected_device = device
                self.load_data()
        if self.editable:
            self.editable = False
                
        return rx.toast("Device updated successfully")
    
    @rx.event
    def copy_device(self, id):
        d = Device(id=id)
        d.copy(id)
        self.initialize()
        self.load_data()

    @rx.event
    def delete_device(self, id):
        d = Device(id=id)
        d.delete(id)
        self.initialize()
        self.load_data()
