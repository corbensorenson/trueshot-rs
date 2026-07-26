# Reflex Photogrammetry Automator

Based on a reflex template

******* Had to alter the utils/serializers.py code of reflex because it could not figure out how to serialize UUID's.  Dont forget to alter this code on upgrades...
     value=str(value) if get_serializer(type(value))==None else value

    And for format.py
    import uuid
    
    # Handle UUID
    if isinstance(value, uuid.UUID):
        return str(value)

***** Had to alter the model.py file because there was infinite recursion when it tried look up relationships on sqlModels, added an extra except: to handle this.
    def dict(self, **kwargs):
        base_fields = {name: getattr(self, name) for name in self.__fields__}
        relationships = {}
        # SQLModel relationships do not appear in __fields__, but should be included if present.
        for name in self.__sqlmodel_relationships__:
           # print(f"Trying {name}")
            try:
                print(f"Trying {name}")
                relationships[name] = self._dict_recursive(getattr(self, name))
            except sqlalchemy.orm.exc.DetachedInstanceError:
                # This happens when the relationship was never loaded and the session is closed.
                continue
            except RecursionError:
                print(f"Recursiion too deep while probling the sqlModel relationship {name}")

This is the base Reflex template - installed when you run `reflex init`.

If you want to use a different template, pass the `--template` flag to `reflex init`.
For example, if you want a more basic starting point, you can run:

```bash
reflex init --template blank
```

## About this Template

This template has the following directory structure:

```bash
├── README.md
├── assets
├── rxconfig.py
└── {your_app}
    ├── __init__.py
    ├── components
    │   ├── __init__.py
    │   ├── navbar.py
    │   └── sidebar.py
    ├── pages
    │   ├── __init__.py
    │   ├── about.py
    │   ├── index.py
    │   ├── profile.py
    │   ├── settings.py
    │   └── table.py
    ├── styles.py
    ├── templates
    │   ├── __init__.py
    │   └── template.py
    └── {your_app}.py
```

See the [Project Structure docs](https://reflex.dev/docs/getting-started/project-structure/) for more information on general Reflex project structure.

### Adding Pages

In this template, the pages in your app are defined in `{your_app}/pages/`.
Each page is a function that returns a Reflex component.
For example, to edit this page you can modify `{your_app}/pages/index.py`.
See the [pages docs](https://reflex.dev/docs/pages/routes/) for more information on pages.

In this template, instead of using `rx.add_page` or the `@rx.page` decorator,
we use the `@template` decorator from `{your_app}/templates/template.py`.

To add a new page:

1. Add a new file in `{your_app}/pages/`. We recommend using one file per page, but you can also group pages in a single file.
2. Add a new function with the `@template` decorator, which takes the same arguments as `@rx.page`.
3. Import the page in your `{your_app}/pages/__init__.py` file and it will automatically be added to the app.
4. Order the pages in `{your_app}/components/sidebar.py` and `{your_app}/components/navbar.py`.


### Adding Components

In order to keep your code organized, we recommend putting components that are
used across multiple pages in the `{your_app}/components/` directory.

In this template, we have a sidebar component in `{your_app}/components/sidebar.py`.

### Adding State

As your app grows, we recommend using [substates](https://reflex.dev/docs/substates/overview/)
to organize your state.

You can either define substates in their own files, or if the state is
specific to a page, you can define it in the page file itself.
