import asyncio
import reflex as rx


class singleSliderState(rx.State):
    dragging: bool = False
    current_focus_actual: int = 50
    sliders_max_actual_value: int = 100
    sliders_min_actual_value: int = 0

    @rx.var
    def current_percent(self) -> str:
        percent_value = (self.current_focus_actual/self.sliders_max_actual_value)*100
        return f"{percent_value}%"
    
    @rx.event
    def start_drag(self):
        self.dragging = True
    
    @rx.event
    def end_drag(self):
        self.dragging = False


    @rx.event
    def handle_drag_test(self):
        if not self.dragging:
            return
        # Increment position by 1 for testing
        self.current_focus_actual = min(
            max(self.current_focus_actual + 1, self.sliders_min_actual_value),
            self.sliders_max_actual_value
        )
    @rx.event(background=True)
    async def handle_drag_js_test(self):
        if not self.dragging:
            return
        print("Dragging")
        
        yield rx.call_script(
            """
            (() => {
                const slider = document.getElementById('slider');
                if (!slider) return null;
                
                // Access mousemove event directly from the document
                document.addEventListener('mousemove', function(e) {
                    const rect = slider.getBoundingClientRect();
                    const mouseX = e.clientX;
                    const mouseY = e.clientY;
                    console.log('MouseX:', mouseX);
                    console.log('MouseY:', mouseY);
                    console.log('Rect:', rect);
                    
                    const percentage = Math.min(100, Math.max(0, ((mouseX - rect.left) / rect.width) * 100));
                    console.log('Calculated percentage:', percentage);
                    return percentage;
                });
            })()
            """,
            callback=self.update_position_percentage,
        )

    @rx.event(background=True)
    async def handle_drag_js(self):
        if not self.dragging:
            return
        print("Dragging")
        
        await asyncio.sleep(0.1)  # Small delay to ensure DOM is ready
        yield rx.call_script(
            """
            (() => {
                const slider = document.getElementById('slider');
                if (!slider) return null;
                
                const rect = slider.getBoundingClientRect();
                const e = window.event || arguments[0];
                const mouseX = e ? e.clientX : null;
                
                const percentage = Math.min(100, Math.max(0, ((mouseX - rect.left) / rect.width) * 100));
                console.log('Calculated percentage:', percentage);
                return percentage;
            })()
            """,
            callback=self._update_position_percentage
        )

    @rx.event(background=True) 
    async def _update_position_percentage(self, percentage):
        print("made it here")
        print(percentage)
        async with self:
            try:
                percentage_float = float(percentage)
                value = int((percentage_float / 100) * self.sliders_max_actual_value)
                self.current_focus_actual = max(
                    self.sliders_min_actual_value,
                    min(self.sliders_max_actual_value, value)
                )
            except (TypeError, ValueError) as e:
                print(f"Error converting percentage: {e}")
        yield


    @rx.event
    def update_position_actual(self, actual: int):
        if actual == '':
            return 
        self.current_focus_actual = max(
            int(self.sliders_min_actual_value),
            min(self.sliders_max_actual_value, int(actual))
        )



def track() -> rx.Component:
    return rx.box(
        background_color="rgba(255, 255, 255, 0.3)",
        height="4px",
        width="100%", 
        position="absolute",
        top="50%",
        transform="translateY(-50%)",
    )

def draggableHandle() -> rx.Component:
    return rx.box(
        background_color="white",
        width="20px",
        height="20px",
        border_radius="50%",
        position="absolute",
        left=singleSliderState.current_percent,
        top="50%",
        transform="translate(-50%, -50%)",
        cursor="pointer",
        z_index="1",
        on_mouse_down=singleSliderState.start_drag,
        on_mouse_up=singleSliderState.end_drag,
        on_mouse_move=singleSliderState.handle_drag_js,
        capture_event=True
    )


def singleSlider() -> rx.Component:
    return rx.hstack(
        rx.box(
            track(),
            draggableHandle(),
            position="relative",
            width="100%",
            height="40px",
            id="slider",  # Important: moved ID here
        ),
        rx.input(
            value=singleSliderState.current_focus_actual,
            type="number",
            on_change=singleSliderState.update_position_actual,
            min_=singleSliderState.sliders_min_actual_value,
            max_=singleSliderState.sliders_max_actual_value,
            width="80px",
        ),
        spacing="4",
    )