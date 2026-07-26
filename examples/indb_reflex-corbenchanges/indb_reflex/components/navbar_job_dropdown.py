import reflex as rx

class navbar_job_dropdow_state(rx.State):
    #would like to have a job class eventually and it just take a refernce to it
    job_currently:bool=False
    job_title:str= None
    info_string:str = None
    progress_bar_percentage:int = None
    show_more:bool = False
    extra_info:str = ''

    @rx.event
    def set_job_title(self, title:str):
        self.job_title = title

    @rx.event
    def set_info_string(self, info:str):
        self.info_string = info

    @rx.event
    def set_progress_bar_percentage(self, percentage:int):
        self.progress_bar_percentage = percentage

    @rx.event
    def update_job(self, perc: int, info: str, extra_info: str = '') -> None:
        """Update the current job's progress and information.
        
        Args:
            percentage: The current progress percentage (0-100)
            info: The main status message
            extra_info: Additional information to display (optional)
        """
        self.set_info_string(info)
        self.set_progress_bar_percentage(perc)
        self.extra_info = extra_info

    @rx.event
    def update_extra_info(self, info:str):
        self.extra_info = info

    @rx.event
    def start_new_job(self, title:str, info:str, bar_percentage:int = 0, extra_info:str = ''):
        self.set_job_title(title)
        self.set_info_string(info) 
        self.progress_bar_percentage = bar_percentage
        self.job_currently = True
        self.extra_info = extra_info

    @rx.event
    def finished_job(self):
        self.job_currently = False
        self.job_title = None
        self.info_string = None
        self.extra_info = ''

    @rx.event
    def toggle_show_more(self):
        self.show_more = not self.show_more

def navbar_job_dropdown()->rx.Component:
    return rx.cond(
        navbar_job_dropdow_state.job_currently,
        rx.card(
            rx.hstack(
                rx.text(navbar_job_dropdow_state.job_title),
                rx.cond(
                    navbar_job_dropdow_state.progress_bar_percentage != -1,
                    rx.hstack(
                        rx.progress(
                            value=navbar_job_dropdow_state.progress_bar_percentage,
                            height="19px",
                            color_scheme='green',
                            width="300px",
                        ),
                        rx.text(f"{navbar_job_dropdow_state.progress_bar_percentage}%", size="3"),
                    ), 
                ),
                rx.text(navbar_job_dropdow_state.info_string, size="3"),
                rx.cond(
                    navbar_job_dropdow_state.extra_info != '',
                    rx.cond(
                        navbar_job_dropdow_state.show_more,
                        rx.icon("arrow-down", on_click=navbar_job_dropdow_state.toggle_show_more),
                        rx.icon("arrow-up", on_click=navbar_job_dropdow_state.toggle_show_more),
                    ),
                ),
            ),
            rx.cond(
                navbar_job_dropdow_state.show_more,
                rx.hstack(
                    rx.text(navbar_job_dropdow_state.extra_info),
                ),
            )
        ),
    )


""" rx.hstack(
    rx.progress(
        value=PAState.acquisition_percent_complete,
        height="19px",
        color_scheme='green',
        width="300px",
    ),
    rx.text(f"{PAState.acquisition_percent_complete}%", size="3"),
    rx.spacer(),
    rx.text(f"Photos: {PAState.photos_taken}", size="3"),
    rx.spacer(),
    rx.text(f"Elapsed: {PAState.time_elapsed}", size="3"),
    rx.spacer(),
    rx.text(f"Remaining: {PAState.time_remaining}", size="3"),
), """
