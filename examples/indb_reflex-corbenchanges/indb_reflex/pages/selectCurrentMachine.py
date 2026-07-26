import reflex as rx
from pga import Machine

from ..states.user_state import *


class selectMachine_state(rx.State):
    data: list[dict] = []
    machine_names: list[str] = []
    @rx.event
    def load_data(self):
        self.machine_names = []
        """Load initial data from database"""
        self.data = [{**result.dict(), 'id': str(result.id)} for result in Machine.all()]
        for machine in self.data:
            self.machine_names.append(machine["name"])

@rx.page("/selectCurrentMachine", title="selectMachine", on_load=selectMachine_state.load_data)
def selectCurrentMachine() -> rx.Component:
    return rx.center(
        rx.card(
            rx.vstack(
                rx.heading("Please Select Current Machine", size="3"),
                rx.hstack(
                    rx.text("Machine: "),
                    rx.select(selectMachine_state.machine_names, default_value=selectMachine_state.machine_names[0], on_change=user_state.set_current_machine),
                ),
            ),
        width="500px",
        ),
        padding_top="300px",

    )
