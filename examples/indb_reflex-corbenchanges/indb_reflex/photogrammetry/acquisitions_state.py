# import reflex as rx
# from typing import Union, Optional, List
# import csv


# class Item(rx.Base):
#     """The item class."""

#     id: int
#     n: int
#     orientation: int
#     description: str


# class AcquisitionsState(rx.State):
#     """The state class."""

#     items: List[Item] = []
#     menu_items: List[str] = []
#     selected_item: Optional[Item] = None


#     total_items: int = 0
#     offset: int = 0
#     limit: int = 12  # Number of rows per page


#     # @rx.var(cache=True, initial_value=[])
#     # def get_menu_items(self):
#     #     self.menu_items=[]
#     #     self.load_entries()
#     #     for item in self.items:
#     #         self.menu_items.append(f"{item.n}, {item.description}")
#     #         # yield f"{item.n}, {item.description}"
#     #     return self.menu_items


#     @rx.var(cache=True, initial_value=[])
#     def get_items(self) -> list[Item]:
#         self.load_entries()
#         return self.items


#     def load_entries(self):
#         with open("acquisitions.csv", mode="r", encoding="utf-8") as file:
#             reader = csv.DictReader(file)
#             self.items = [Item(**row) for row in reader]
#             self.total_items = len(self.items)
#             mitems=[]
#             for item in self.items:
#                 mitems.append(f"{item.n} -  O({item.orientation}) -  {item.description}")
#             self.menu_items=mitems

