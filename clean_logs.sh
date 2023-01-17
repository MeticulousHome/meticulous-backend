#!/bin/bash
# Comprobar si la carpeta existe
if [ ! -d "/home/meticulous/meticulous-raspberry-setup/backend_for_esp32/logs" ]; then
  # Crear carpeta si no existe
  mkdir /home/meticulous/meticulous-raspberry-setup/backend_for_esp32/logs
fi

# # Obtener el número de archivos en la carpeta
# num_files=$(ls -l /home/meticulous/meticulous-raspberry-setup/backend_for_esp32/logs | wc -l)

# # Comprobar si hay más de 30 archivos
# if [ $num_files -gt 31 ]; then
#   # Moverse a la carpeta
#   cd /home/meticulous/meticulous-raspberry-setup/backend_for_esp32/logs
#   # Crear una lista de los archivos segun el tiempo, mostrar aquellos que esten despues de las primeras 31 lines(una co> 
#   ls -t | tail -n +31 | xargs rm
# fi
