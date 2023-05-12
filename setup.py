from setuptools import setup, find_packages
from version import VERSION

setup(
    name="meticulous_backend",
    version=VERSION,
    packages=find_packages(),
    install_requires=[
        "bidict==0.22.0",
        "evdev==1.6.0",
        "gpiod==1.5.3",
        "pynput==1.7.6",
        "pyserial==3.5",
        "python-dotenv==0.21.0",
        "python-engineio==4.3.4",
        "python-socketio==5.7.2",
        "python-xlib==0.31",
        "RPi.GPIO==0.7.1",
        "six==1.16.0",
        "threaded==4.1.0",
        "tornado==6.2",
    ],
    include_package_data=True,
    entry_points={
        "console_scripts": [
            "meticulous-backend = meticulous_backend.back:main"
        ],
    },
    # Add any additional metadata or options
)
