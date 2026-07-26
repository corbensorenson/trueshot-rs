import json
from typing import Any
import reflex as rx
from indb_reflex.states.photoAcquisition_state import PAState

class focusSliderState(rx.State):
    dragging: bool = False
    manual_low = 5
    manual_mid = 10
    manual_high = 20



    @rx.var
    def photo_stack_start_end_coverage_percent_width(self) -> str:
        width = (PAState.focus_end_z - PAState.focus_start_z) / PAState.lens_end_focus_z * 100
        return f"{width}%"
    
    @rx.var
    def photo_stack_start_end_coverage_percent_left_offset(self) -> str:
        left = PAState.focus_start_z / PAState.lens_end_focus_z * 100
        return f"{left}%"

    @rx.var
    def current_percent(self) -> str:
        percent_value = (PAState.supposed_focus_z/PAState.lens_end_focus_z)*100
        return f"{percent_value}%"
    
    @rx.event
    def set_manual_low_incriment(self, value: int):
        self.manual_low = value
    
    @rx.event
    def set_manual_mid_incriment(self, value: int):
        self.manual_mid = value

    @rx.event
    def set_manual_high_incriment(self, value: int):
        self.manual_high = value

    
    @rx.event
    def start_drag(self):
        self.dragging = True
    
    @rx.event
    def end_drag(self):
        self.dragging = False

    @rx.background
    async def handle_drag_js_test(self):
        if not self.dragging:
            return
        print("Dragging - calling script")
        yield rx.call_script(
            """
            (() => {
                const slider = document.getElementById('slider');
                const input = document.getElementById('slider-input');
                if (!slider) {
                    console.log("No slider found.");
                    return null;
                }
                return new Promise((resolve) => {
                    const handleMouseMove = (e) => {
                        const rect = slider.getBoundingClientRect();
                        const mouseX = e.clientX;
                        const percentage = Math.min(100, Math.max(0, ((mouseX - rect.left) / rect.width) * 100));
                        console.log('Calculated percentage:', percentage);
                        resolve(JSON.stringify({ percentage }));
                        document.removeEventListener('mousemove', handleMouseMove);
                    };
                    document.addEventListener('mousemove', handleMouseMove);
                });
            })()
            """,
            callback=self._update_position_percentage,
        )
    @rx.background
    async def _update_position_percentage(self, data: str):
        """Callback that receives JSON data from handle_drag_js_test and parses it."""
        print("Callback triggered: _update_position_percentage")
        print("Raw data:", data)
        try:
            # Parse JSON
            obj = json.loads(data or "{}")
            percentage_str = obj.get("percentage", 0)
            percentage_float = float(percentage_str)

            value = int((percentage_float / 100) * PAState.lens_end_focus_z)
            PAState.supposed_focus_actual = max(
                PAState.sliders_min_actual_value,
                min(PAState.lens_end_focus_z, value)
            )
        except (TypeError, ValueError) as e:
            print(f"Error converting percentage: {e}")

    @rx.event
    def update_position_actual(self, actual: int):
        if actual == '':
            return 
        PAState.set_supposed_focus_z(max(
            int(0),
            min(PAState.lens_end_focus_z, int(actual))
        ))