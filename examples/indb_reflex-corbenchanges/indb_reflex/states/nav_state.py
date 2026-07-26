import reflex as rx
from reflex.page import get_decorated_pages

class nav_state(rx.State):
    currentPage = ""

    @rx.var
    def get_current_page_route(self) -> str:
        return self.router.page.path
    
    def get_current_page_url(self):
        return self.router.page.raw_path
    
    def get_current_page_title(self, path=rx.State.router.page.path):
        pages = get_decorated_pages()
        for page in pages:
            if page.path == path:
                return page.title
    @rx.var
    def current_url(self) -> str:
        return self.router.page.full_raw_path