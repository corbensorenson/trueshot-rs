import reflex as rx
from pga import Machine
from pga import Device
from sqlmodel import SQLModel, select

class user_state(rx.State):
    logged_in: bool = False
    authenticated:str = None

    current_machine:Machine=None
    current_machine_name:str = ""

    current_camera:Device=None
    current_camera_name:str = ""
    camera_names:list[str] = []

    current_turntable:Device=None
    current_turntable_name:str= ""
    turntable_names:list[str] = []

    current_arm:Device=None
    current_arm_name:str = ""
    arm_names:list[str] = []

    @rx.event
    def load_data(self):
        self.camera_names = self.get_camera_names()
        self.turntable_names = self.get_turntable_names()
        self.arm_names = self.get_arm_names()

    @rx.var
    def camera_chosen(self) -> bool:
        return self.current_camera != None
    
    @rx.var
    def turntable_chosen(self) -> bool:
        return self.current_turntable != None
    
    @rx.var
    def arm_chosen(self) -> bool:
        return self.current_arm != None
    
    @rx.event
    def set_current_camera(self, camera_name:str):
        with rx.session() as session:
            search_column = getattr(Device, "name")
            # For string columns, use ILIKE
            query = select(Device).where(
                search_column.ilike(f"%{camera_name.lower()}%")
            )
            camera = session.exec(query).first()
            self.current_camera_name = camera_name
            self.current_camera = camera

    @rx.event
    def set_current_turntable(self, turntable_name:str):
        with rx.session() as session:
            search_column = getattr(Device, "name")
            # For string columns, use ILIKE
            query = select(Device).where(
                search_column.ilike(f"%{turntable_name.lower()}%")
            )
            turntable = session.exec(query).first()
            self.current_turntable_name = turntable_name
            self.current_turntable = turntable

    @rx.event
    def set_current_arm(self, arm_name:str):
        with rx.session() as session:
            search_column = getattr(Device, "name")
            # For string columns, use ILIKE
            query = select(Device).where(
                search_column.ilike(f"%{arm_name.lower()}%")
            )
            arm = session.exec(query).first()
            self.current_arm_name = arm_name
            self.current_arm = arm

    @rx.event
    def set_current_machine(self, machine_name:str):
        with rx.session() as session:
            search_column = getattr(Machine, "name")
            # For string columns, use ILIKE
            query = select(Machine).where(
                search_column.ilike(f"%{machine_name.lower()}%")
            )
            machine = session.exec(query).first()
        self.current_machine_name = machine_name
        self.current_machine = machine
        if self.logged_in:
            return rx.redirect("/photoAcquisition")
        else:
            return rx.redirect("/login")
        
    def get_camera_names(self) -> list[str]:
        namesList:list[str] = []
        with rx.session() as session:
            query = select(Device)
            # Filter out all jobs but those that aren't started yet
            query = query.where(Device.category == "camera")
            results = session.exec(query).all()
            data = [{**result.dict(), 'id': str(result.id)} for result in results]
            for camera in data:
                namesList.append(camera["name"])
            return namesList
    
    def get_turntable_names(self) -> list[str]:
        namesList:list[str] = []
        with rx.session() as session:
            query = select(Device)
            # Filter out all jobs but those that aren't started yet
            query = query.where(Device.category == "turntable")
            results = session.exec(query).all()
            data = [{**result.dict(), 'id': str(result.id)} for result in results]
            for turntable in data:
                namesList.append(turntable["name"])
            return namesList
    
    def get_arm_names(self) -> list[str]:
        namesList:list[str] = []
        with rx.session() as session:
            query = select(Device)
            # Filter out all jobs but those that aren't started yet
            query = query.where(Device.category == "arm")
            results = session.exec(query).all()
            data = [{**result.dict(), 'id': str(result.id)} for result in results]
            for arm in data:
                namesList.append(arm["name"])
        return namesList

    

    @rx.event
    def simple_check_login(self):
        if self.current_machine == None:
            return rx.redirect("/selectCurrentMachine")
        if not self.logged_in:
            return rx.redirect("/login")

    #ignore for now
    @rx.event
    def check_auth(self):
        # Check if user is authenticated
        self.authenticated = None
        if not self.authenticated:
            return rx.redirect("/login")
    

    #temporarily set to True for testing
    @rx.event
    def login(self):
        self.logged_in = not self.logged_in
        if self.logged_in:
            return rx.redirect("/photoAcquisition")

    def is_logged_in(self):
        return self.logged_in 