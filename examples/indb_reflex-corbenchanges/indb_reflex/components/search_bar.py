import reflex as rx


def search_bar(state) -> rx.Component:
    return rx.hstack(
            rx.spacer(),
            rx.input(
                type="String",
                value=state.search_by_term,
                on_change=lambda e: state.set_search_by_term(e), 
                placeholder="Search....",
                margin_right="10px",
                width="200px",
            ),
            rx.select(
                    state.search_by_options,
                    size="2",
                    value=state.search_by_current,
                    on_change=state.set_search_by_current,
                    width="140px"
                ),
            rx.button("search", on_click=state.search_data),
            rx.button("clear search", on_click=state.clear_search),
            rx.spacer(),
            width = "100%"
        )