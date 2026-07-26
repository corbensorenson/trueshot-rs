# pga - photogrammetry automation

Database models and hardware interfaces to acquiring photographs from a camera and turntable

pixi install most dependencies
    --pixi was unable to install libgphoto2 (not present for apple silicone in conda forge).  Use brew install libgphoto2 and brew install pkg-config for apple silicone

Models: sqlModel

Migrations: alembic
create migration file: python -m alembic revision --autogenerate -m 'initial migration'
execute migration: python -m alembic upgrade head