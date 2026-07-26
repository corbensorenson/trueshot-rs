import reflex as rx


def subject_heading( title: str, icon: str) -> rx.Component:
    return rx.hstack(
        rx.icon(icon) if icon else None,
        rx.heading(title, size="4"),
        align="center",
    )

def text_input(label: str, value: None, on_change: None, vertical=True, width='200px') -> rx.Component:
    if(vertical):
        return rx.vstack(
            rx.text(label, size="3", weight="bold",),
            rx.input(value=value, on_change=on_change, width=width),
            spacing="2",
            width=width
        )
    else:
        return rx.hstack(
            rx.text(label, size="3", weight="bold"),
            rx.input(value=value, on_change=on_change, debounce_timeout=500),
            spacing="2",
            width=width
        )

def text_value(label: str, value: None,vertical=True, width='200px') -> rx.Component:
    if(vertical):
        return rx.vstack(
            rx.text(label, size="3", weight="bold"),
            rx.text(value, width=width),
            spacing="2",
            width=width
        )
    else:
        return rx.hstack(
            rx.text(label, size="3", weight="bold"),
            rx.input(value),
            spacing="2",
            width=width
        )

def option_input(label: str, value: None, options: None, on_change: None, vertical=True, width='200px', space = "2") -> rx.Component:
    if(vertical):
        return rx.vstack(
            rx.heading(label, size="3"),
            rx.select(
                options,
                size="2",
                value=value,
                on_change=on_change,
                width=width
            ),
            spacing=space,
            width="100%",
        )
    else:
        return rx.hstack(
            rx.heading(label, size="3"),
            rx.select(
                options,
                size="2",
                value=value,
                on_change=on_change,
                width=width
            ),
            spacing=space,
            width=width
        )
    
def new_item_dialog(title: str, msg: str, state=None, value=None, action=None, icon='plus') -> rx.Component:

    m=getattr(state, value)
    callb=getattr(state, f"set_{value}")

    return rx.dialog.root(
        rx.dialog.trigger(rx.button(rx.icon(icon))),
        rx.dialog.content(
            rx.dialog.title(title),
            rx.dialog.description(
                msg,
                size="2",
                margin_bottom="16px",
            ),
            rx.flex(
                rx.input(
                    value=m,
                     on_change=callb,
                    placeholder="Enter name"
                ),
                direction="column",
                spacing="3"
            ),
            rx.flex(
                rx.dialog.close(
                    rx.button(
                        "Cancel",
                        color_scheme="gray",
                        variant="soft",
                    ),
                ),
                rx.dialog.close(
                    rx.button(
                        "Save",
                        on_click=action,
                    ),
                ),
                spacing="3",
                margin_top="16px",
                justify="end",
            ),
        )
    )

class EditableText(rx.ComponentState):
    text: str = "Click to edit"
    original_text: str
    editing: bool = False

    def start_editing(self, original_text: str):
        self.original_text = original_text
        self.editing = True

    def stop_editing(self):
        self.editing = False
        self.original_text = ""

    @classmethod
    def get_component(cls, **props):
        # Pop component-specific props with defaults before passing **props
        value = props.pop("value", cls.text)
        on_change = props.pop("on_change", cls.set_text)
        cursor = props.pop("cursor", "pointer")

        # Set the initial value of the State var.
        initial_value = props.pop("initial_value", None)
        if initial_value is not None:
            # Update the pydantic model to use the initial value as default.
            cls.__fields__["text"].default = initial_value

        # Form elements for editing, saving and reverting the text.
        edit_controls = rx.hstack(
            rx.text_area(
                value=value,
                on_change=on_change,
                debounce_timeout=20000,
                **props,
            ),
            rx.icon_button(
                rx.icon("x"),
                on_click=[
                    on_change(cls.original_text),
                    cls.stop_editing,
                ],
                type="button",
                color_scheme="red",
            ),
            rx.icon_button(rx.icon("check")),
            align="center",
            height="100%",
            width="100%",
        )

        return rx.cond(
            cls.editing,
            rx.form(
                edit_controls,
                on_submit=lambda _: cls.stop_editing(),
            ),
            rx.text(
                value,
                on_click=cls.start_editing(value),
                cursor=cursor,
                **props,
            ),
        )

editable_text = EditableText.create
