import reflex as rx

@rx.page(route="/forgotPassword", title="Forgot Password")
def forgotPassword() -> rx.Component:
    return rx.center(
        rx.card(
            rx.vstack(
                rx.center(
                    rx.image(
                        src="/logo.jpg",
                        width="2.5em",
                        height="auto",
                        border_radius="25%",
                    ),
                    rx.heading(
                        "So you forgot your password?",
                        size="6",
                        as_="h2",
                        text_align="center",
                        width="100%",
                    ),
                    direction="column",
                    spacing="5",
                    width="100%",
                ),
                rx.vstack(
                    rx.text(
                        "Email address",
                        size="3",
                        weight="medium",
                        text_align="left",
                        width="100%",
                    ),
                    rx.input(
                        placeholder="user@reflex.dev",
                        type="email",
                        size="3",
                        width="100%",
                    ),
                    justify="start",
                    spacing="2",
                    width="100%",
                ),
                
                rx.button("Reset Password", size="3", width="100%"),
                
                rx.center(
                    rx.link("Oh? so you remember your password now", href="/login", size="3"),
                    width="100%",
                ),
                
                spacing="6",
                width="100%",
            ),
            size="4",
            max_width="28em",
            width="100%",
            margin_top="16vh",
        )
    )