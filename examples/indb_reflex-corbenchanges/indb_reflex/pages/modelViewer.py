import reflex as rx

#i would like to be able to pass as an argument the model in the route/header somehow
#currently am passing it like f'/modelViewer/?id={id}' into the url
#so how do i go about retrieving that info.....

class ModelViewerState(rx.State):
    @rx.var
    def model_id(self) -> str:
        # Access query parameters from router
        if 'id=' in self.router.page.raw_path:
            return self.router.page.raw_path.split('id=')[1]
        else:
            return None

@rx.page(route="/modelViewer", title="ModelViewer")
def modelViewer() -> rx.Component:
    return rx.hstack(
        rx.text("Model ID: "),
        rx.text(ModelViewerState.model_id)
    )