"""Navbar component for the app."""

from .. import styles

import reflex as rx
from ..states.nav_state import nav_state
from ..states.user_state import user_state
from .navbar_job_dropdown import navbar_job_dropdown


def current_page_title() -> rx.Component:
    #from reflex.page import get_decorated_pages
    #pages = get_decorated_pages()
    #title = nav_state.currentPage
    #print(pages)
    return rx.match(
        rx.State.router.page.path,
        ("/", rx.text("Pipeline", size="6")),
        ("/machines", rx.text("Machines", size="6")),
        ("/devices", rx.text("Devices", size="6")),
        ("/implementationManager", rx.text("Implementation Manager", size="6")),
        ("/jobManager", rx.text("Job Manager", size="6")),
        ("/photoAcquisition", rx.text("Photo Acquisition", size="6")),
        ("/models", rx.text("Models", size="6")),
        ("/userSettings", rx.text("User Settings", size="6")),
        ("/adminSettings", rx.text("Admin Settings", size="6")),
        ("/about", rx.text("About", size="6")),
        rx.text("unknown page")
    )
    """ for page in pages:
        print(page["route"])
        print(nav_state.get_current_page_route)
        #rx.cond(page["route"] == rx.State.router.page.path, title = page["title"], title = "Rhoton PGA tool")
        if page["route"] == rx.State.router.page.path:
            title =  page["title"]
    return rx.text(title, size="6") """

def menu_item_icon(icon: str) -> rx.Component:
    return rx.icon(icon, size=20)


def menu_item(text: str, url: str) -> rx.Component:
    """Menu item.

    Args:
        text: The text of the item.
        url: The URL of the item.

    Returns:
        rx.Component: The menu item component.
    """
    # Whether the item is active.
    active = (rx.State.router.page.path == url.lower()) | (
        (rx.State.router.page.path == "/") & text == "Overview"
    )
    """ if active:
        nav_state.currentPage = text """

    return rx.link(
        rx.hstack(
            rx.match(
                text,
                ("Job Manager", menu_item_icon("home")),
                ("Photo Acquisition", menu_item_icon("camera")),
                ("Models", menu_item_icon("table-2")),
                ("User Settings", menu_item_icon("user")),
                ("Admin Settings", menu_item_icon("settings")),
                ("Pipeline", menu_item_icon("cable")),
                ("Implementation Manager", menu_item_icon("fingerprint")),
                ("Machines", menu_item_icon("cpu")),
                ("Devices", menu_item_icon("cpu")),
                menu_item_icon("layout-dashboard"),
            ),
            rx.text(text, size="4", weight="regular"),
            color=rx.cond(
                active,
                styles.accent_text_color,
                styles.text_color,
            ),
            style={
                "_hover": {
                    "background_color": rx.cond(
                        active,
                        styles.accent_bg_color,
                        styles.gray_bg_color,
                    ),
                    "color": rx.cond(
                        active,
                        styles.accent_text_color,
                        styles.text_color,
                    ),
                    "opacity": "1",
                },
                "opacity": rx.cond(
                    active,
                    "1",
                    "0.95",
                ),
            },
            align="center",
            border_radius=styles.border_radius,
            width="100%",
            spacing="2",
            padding="0.35em",
        ),
        underline="none",
        href=url,
        width="100%",
    )

def login_button() -> rx.Component:
    """Login or Logout button based on user's login state.

    Returns:
        The login or logout button component.
    """
    return rx.cond(
        user_state.logged_in,
        rx.button(
            "Log out",
            on_click=lambda: user_state.login(),
            align="center",
            border_radius=styles.border_radius,
            width="50%",
            spacing="2",
            padding="0.35em",
        ),
        rx.link(
            rx.button(
                "Log in",
                align="center",
                border_radius=styles.border_radius,
            ),
            href="/login",
        ),
    )

def navbar_footer() -> rx.Component:
    """Navbar footer.

    Returns:
        The navbar footer component.
    """
    return rx.hstack(
        rx.link(
            rx.text("Docs", size="3"),
            href="https://reflex.dev/docs/getting-started/introduction/",
            color_scheme="gray",
            underline="none",
        ),
        rx.spacer(),
        rx.link(
            rx.text("About", size="3"),
            href="/about",
            color_scheme="gray",
            underline="none",
        ),
        rx.spacer(),
        rx.color_mode.button(style={"opacity": "0.8", "scale": "0.95"}),
        justify="start",
        align="center",
        width="100%",
        padding="0.35em",
    )

def navbar_header() -> rx.Component:
    return rx.hstack(
                rx.spacer(),
                login_button(),
                rx.spacer(),
                rx.drawer.close(rx.icon(tag="x")),
                justify="end",
                width="100%",
            )

def menu_button() -> rx.Component:
    # Get all the decorated pages and add them to the menu.
    from reflex.page import get_decorated_pages

    # The ordered page routes.
    ordered_page_routes = [
        "/",
        "/devices",
        "/machines",
        "/jobManager",
        "/photoAcquisition",
        "/models",
        "/userSettings",
        "/implementationManager",
        "/adminSettings",
    ]

    # Get the decorated pages.
    pages = get_decorated_pages()

    filtered_pages = [page for page in pages if page["route"] in ordered_page_routes]


    # Include all pages even if they are not in the ordered_page_routes.
    ordered_pages = sorted(
        filtered_pages,
        key=lambda page: ordered_page_routes.index(page["route"])
    )

    return rx.drawer.root(
        rx.drawer.trigger(
            rx.icon("align-justify"),
        ),
        rx.drawer.overlay(z_index="5"),
        rx.drawer.portal(
            rx.drawer.content(
                rx.vstack(
                    navbar_header(),
                    rx.divider(),
                    *[
                        menu_item(
                            text=page.get(
                                "title", page["route"].strip("/").capitalize()
                            ),
                            url=page["route"],
                        )
                        for page in ordered_pages
                    ],
                    rx.spacer(),
                    navbar_footer(),
                    spacing="4",
                    width="100%",
                ),
                top="auto",
                right="0",
                height="100%",
                width="20em",
                padding="1em",
                bg=rx.color("gray", 1),
            ),
            width="100%",
        ),
        direction="left",

    )


def navbar() -> rx.Component:
    """The navbar.

    Returns:
        The navbar component.
    """
    
    return rx.el.nav(
        rx.hstack(
            # The logo.
            # rx.color_mode_cond(
            #     rx.image(src="/reflex_black.svg", height="1em"),
            #     rx.image(src="/reflex_white.svg", height="1em"),
            # ),
            current_page_title(),
            menu_button(),
            rx.spacer(),
            navbar_job_dropdown(),
            rx.spacer(),
            rx.vstack(
                rx.heading("Rhoton PGA tool", size="7", padding_bottom="0px", margin_bottom="0px", padding_top="6px"),
                rx.text(f"Machine: {user_state.current_machine_name}", margin_top="0px", padding_top="0px", padding_bottom="0px", margin_bottom="0px"),
                spacing="0",
            ),
            
            align="center",
            width="100%",
            #padding_y=".5em",
            padding_x=["1em", "1em", "2em"],
        ),
        #display=["block", "block", "block", "block", "block", "block"],
        position="sticky",
        background_color=rx.color("gray", 1),
        top="0px",
        z_index="5",
        border_bottom=styles.border,
    )
