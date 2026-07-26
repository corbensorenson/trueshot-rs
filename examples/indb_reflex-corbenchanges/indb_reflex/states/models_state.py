from typing import Optional
import os
from ..templates import template
from reflex_ag_grid import ag_grid
from sqlmodel import SQLModel, select
import reflex as rx
from pga import MeshModel
from pga import PhotoSequence
from pga import CameraPreset
from ..photogrammetry.widgets import *
from sqlalchemy.dialects.postgresql import UUID

class models_state(rx.State):
    """State for managing the AG Grid and dialog interactions"""
    data: list[dict] = []
    selected_model: MeshModel = None
    selected_model_id: str = None
    show_dialog: bool = False
    search_by_term: str = ''
    search_by_options: list[str]= ["name", "id", "created_at", "number"]
    search_by_current: str = "name"
    editable:bool = False

    thumbnail_location_url = "http://localhost:8000/_upload/thumbnails/"

    @rx.event
    def initialize(self):
        self.search_by_term = ""
        self.search_by_current = "name"
        self.selected_model = None
        self.show_dialog = False
        self.editable = False
        self.load_data()


    @rx.var
    def thumbnail_location(self) -> str:
        return str(rx.get_upload_dir() / f"thumbnails/{self.selected_model.id}.jpg")
    
    @rx.var
    def thumbnail_location_string(self) -> str:
        return self.thumbnail_location_url+f"{self.selected_model.id}.jpg"
    
    @rx.var
    def thumbnail_exists(self) -> bool:
        return os.path.exists(self.thumbnail_location)
    
    @rx.var(cache=False)
    def has_selected_model(self) -> bool:
        return self.selected_model is not None
    
    @rx.event
    def toggle_editable(self):
        self.editable = not self.editable

    @rx.event
    def set_search_by_current(self, term:str):
        self.search_by_current = term

    @rx.event
    def set_search_by_term(self, term:str):
        self.search_by_term = term

    @rx.event
    def load_data(self):
        """Load initial data from database"""
        self.data = [{**result.dict(), 'id': str(result.id)} for result in MeshModel.all()] 

    @rx.event
    def search_data(self):
        with rx.session() as session:
            try:
                if self.search_by_term:
                    search_column = getattr(MeshModel, self.search_by_current)
                    
                    # Handle different column types
                    if self.search_by_current == "number":
                        # For integer columns, cast the search term to integer
                        try:
                            search_value = int(self.search_by_term)
                            query = select(MeshModel).where(search_column == search_value)
                        except ValueError:
                            # If search term isn't a valid integer, return no results
                            self.data = []
                            return
                    else:
                        # For string columns, use ILIKE
                        query = select(MeshModel).where(
                            search_column.ilike(f"%{self.search_by_term.lower()}%")
                        )
                    
                    results = session.exec(query).all()
                    self.data = [result.dict() for result in results]
                    # Convert UUID to string for display
                    for model in self.data:
                        model['id'] = str(model['id'])
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
            model = MeshModel(**model_data)
            session.merge(model)
            session.commit()
            self.load_data()
        yield rx.toast(f"Cell value changed, Row: {row}, Column: {col_field}, New Value: {new_value}")

    @rx.event
    def open_dialog(self, row_data: dict):
        if row_data:
            with rx.session() as session:
                # Query the full model from the database
                self.selected_model_id = row_data['data']['id']
                model = session.exec(
                    select(MeshModel).where(MeshModel.id == self.selected_model_id)
                ).first()
                if model:
                    self.selected_model = model
                    self.editable = False
                    yield associated_sequences_table_state.load_data(self.selected_model_id)
                    #self.show_dialog = True    

    @rx.event
    def close_dialog(self):
        """Close the dialog"""
        self.selected_model = None
        self.show_dialog = False

    @rx.event
    def delete_model(self, id):
        m = MeshModel(id=id)
        m.delete(id)
        self.initialize()
        self.load_data()


class associated_sequences_table_state(rx.State):
    data: list[dict] = [] # Change models to data to match AG Grid expectations
    selected_sequence: PhotoSequence = None
    selected_sequence_id: str = None
    selected_camera_preset: CameraPreset = None
    show_dialog: bool = False

    @rx.event
    def load_data(self, model_id: str):
        if model_id:  # Check that model and ID exist
            with rx.session() as session:
                # Get a fresh copy of the model from the session
                current_model = session.exec(
                    select(MeshModel).where(MeshModel.id == model_id)
                ).first()
                if current_model:
                    query = select(PhotoSequence).where(
                        PhotoSequence.mesh_model== current_model
                    )
                    results = session.exec(query).all()
                    self.data = [result.dict() for result in results]
                    #print(self.data)
                    for seq in self.data:
                        seq['id'] = str(seq['id'])
                        seq['camera_preset_id'] = str(seq['camera_preset_id'])

    @rx.event
    def cell_value_changed(self, row, col_field, new_value):
        with rx.session() as session:
            # Update the specific row
            seq_data = self.data[row]
            seq_data[col_field] = new_value
            seq = PhotoSequence(**seq_data)
            session.merge(seq)
            session.commit()
            # Refresh data after update
            self.load_data()
        yield rx.toast(f"Cell value changed, Row: {row}, Column: {col_field}, New Value: {new_value}")

    @rx.event
    def open_dialog(self, row_data: dict):
        if row_data:
            #print(row_data)
            with rx.session() as session:
                # Query the full model from the database
                self.selected_sequence_id = str(row_data['data']['id'])
                sequence = session.exec(
                    select(PhotoSequence).where(PhotoSequence.id == self.selected_sequence_id)
                ).first()
                if sequence:
                    self.selected_sequence = sequence
                    print(self.selected_sequence.camera_preset)
                    """ self.selected_camera_preset = session.exec(
                        select(CameraPreset).where(CameraPreset.name == self.selected_sequence.camera_preset.name)
                                ).first() """
                    self.show_dialog = True    

    @rx.event
    def close_dialog(self):
        """Close the dialog"""
        self.selected_sequence = None
        self.show_dialog = False