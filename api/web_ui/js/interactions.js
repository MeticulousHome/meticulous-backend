// Initialize motor control switches and sliders
let motorPower = 0;
let heaterPower = 0;
let motorEnabled = false;
let heaterEnabled = false;

function start_interactions() {
  initialize_toggle_switch_heater();
  initialize_slider_control_heater();
  initialize_toggle_switch_motor();
  initialize_slider_control_motor();

  // Event handlers for additional motor buttons
  document
    .getElementById("backwardButton")
    .addEventListener("click", () => console.log("backward"));
  document
    .getElementById("forwardButton")
    .addEventListener("click", () => console.log("forward"));

  initialize_menu_var_som_controls();
  // updateCurrentTime();
}

// Initialize heater toggle switch
function initialize_toggle_switch_heater() {
  const enable_switch_heater = document.getElementById("enableSwitchHeater");
  const toggle_slider_heater = document.getElementById("toggleSliderHeater");

  enable_switch_heater.addEventListener("click", () => {
    if (!is_block) {
      heaterEnabled = !heaterEnabled;
      update_switch_appearance(
        heaterEnabled,
        toggle_slider_heater,
        enable_switch_heater
      );
      console.log("Heater switch toggled:", heaterEnabled);
      if (heaterEnabled) {
        send_actuator_data("heater", heaterPower, heaterEnabled, false);

        // Temporizador para desactivar el calentador después de 2 segundos
        setTimeout(() => {
          heaterEnabled = false;
          update_switch_appearance(
            heaterEnabled,
            toggle_slider_heater,
            enable_switch_heater
          );
          send_actuator_data("heater", 0, heaterEnabled, false);
          console.log("Heater switch auto-off after 2 seconds");
        }, 2000);
      }
    }
  });

  update_switch_appearance(
    heaterEnabled,
    toggle_slider_heater,
    enable_switch_heater
  );
}
// Update switch appearance
function update_switch_appearance(is_switch_on, toggle_slider, switch_element) {
  if (!is_switch_on) {
    toggle_slider.style.transform = "translateX(24px)";
    switch_element.style.backgroundColor = "#6D6D6B";
  } else {
    toggle_slider.style.transform = "translateX(0)";
    switch_element.style.backgroundColor = "#fdc352";
  }
}

// Initialize heater slider control
function initialize_slider_control_heater() {
  const slider_thumb = document.getElementById("sliderThumbHeater");
  const slider_bar = document.getElementById("sliderBarHeater");
  const current_power = document.getElementById("currentPowerHeater");

  slider_thumb.onmousedown = function (event) {
    event.preventDefault();
    if (!is_block){
    start_slider_drag(event, slider_thumb, slider_bar, current_power);
    }
  };

  slider_thumb.ondragstart = function () {
    return false;
  };
}

// Start slider drag
function start_slider_drag(event, thumb, bar, power_display) {
  let shift_x = event.clientX - thumb.getBoundingClientRect().left;
  let bar_rect = bar.getBoundingClientRect();

  document.addEventListener("mousemove", on_mouse_move);
  document.addEventListener("mouseup", on_mouse_up);

  function on_mouse_move(event) {
    update_slider_position(event, shift_x, bar_rect, thumb, power_display);
  }

  function on_mouse_up() {
    document.removeEventListener("mouseup", on_mouse_up);
    document.removeEventListener("mousemove", on_mouse_move);
  }
}

// Update slider position
function update_slider_position(
  event,
  shift_x,
  bar_rect,
  thumb,
  power_display
) {
  let new_left = event.clientX - shift_x - bar_rect.left;
  new_left = Math.max(
    0,
    Math.min(new_left, bar_rect.width - thumb.offsetWidth)
  );
  thumb.style.left = new_left + "px";
  heaterPower = Math.round(
    (new_left / (bar_rect.width - thumb.offsetWidth)) * 100
  );
  power_display.textContent = `${heaterPower}%`;
  console.log("Heater slider position:", heaterPower);
}

// Initialize motor toggle switch
function initialize_toggle_switch_motor() {
  const enable_switch_motor = document.getElementById("enableSwitchMotor");
  const toggle_slider_motor = document.getElementById("toggleSliderMotor");

  enable_switch_motor.addEventListener("click", () => {
    motorEnabled = !motorEnabled;
    update_switch_ui(motorEnabled, toggle_slider_motor, enable_switch_motor);
    console.log("Motor switch toggled:", motorEnabled);
  });

  update_switch_ui(motorEnabled, toggle_slider_motor, enable_switch_motor);
}

// Update motor switch UI
function update_switch_ui(
  is_switch_on,
  toggle_slider_motor,
  enable_switch_motor
) {
  if (!is_switch_on) {
    toggle_slider_motor.style.transform = "translateX(24px)";
    enable_switch_motor.style.backgroundColor = "#6D6D6B";
  } else {
    toggle_slider_motor.style.transform = "translateX(0)";
    enable_switch_motor.style.backgroundColor = "#fdc352";
  }
}

// Initialize motor slider control
function initialize_slider_control_motor() {
  const slider_thumb_motor = document.getElementById("sliderThumbMotor");
  const slider_bar_motor = document.getElementById("sliderBarMotor");
  const current_power_motor = document.getElementById("currentPowerMotor");

  slider_thumb_motor.addEventListener("mousedown", (event) => {
    event.preventDefault();
    handle_slider_movement(
      event,
      slider_thumb_motor,
      slider_bar_motor,
      current_power_motor
    );
  });

  slider_thumb_motor.ondragstart = () => false;
}

// Handle slider movement
function handle_slider_movement(
  event,
  slider_thumb_motor,
  slider_bar,
  current_power_display
) {
  let shift_x = event.clientX - slider_thumb_motor.getBoundingClientRect().left;
  let slider_bar_rect = slider_bar.getBoundingClientRect();

  const on_mouse_move = (event) => {
    move_slider(
      event,
      shift_x,
      slider_bar_rect,
      slider_thumb_motor,
      current_power_display
    );
  };

  document.addEventListener("mousemove", on_mouse_move);
  document.addEventListener("mouseup", () => {
    document.removeEventListener("mouseup", on_mouse_move);
    document.removeEventListener("mousemove", on_mouse_move);
  });
}

// Move slider
function move_slider(
  event,
  shift_x,
  slider_bar_rect,
  slider_thumb_motor,
  current_power_display
) {
  let new_left = event.clientX - shift_x - slider_bar_rect.left;
  new_left = Math.max(
    0,
    Math.min(new_left, slider_bar_rect.width - slider_thumb_motor.offsetWidth)
  );
  motorPower = Math.round(
    (new_left / (slider_bar_rect.width - slider_thumb_motor.offsetWidth)) * 100
  );
  slider_thumb_motor.style.left = new_left + "px";
  update_slider_text(
    new_left,
    slider_bar_rect,
    slider_thumb_motor,
    current_power_display
  );
  console.log("Motor slider position:", motorPower);
}

// Update slider text
function update_slider_text(
  new_left,
  slider_bar_rect,
  slider_thumb_motor,
  current_power_display
) {
  let percentage = Math.round(
    (new_left / (slider_bar_rect.width - slider_thumb_motor.offsetWidth)) * 100
  );
  current_power_display.textContent = percentage + "%";
}

function send_actuator_data(actuator, power, enable, direction) {
  const jsonData = {
    actuator: actuator,
    power: power,
    enable: enable,
    direction: direction,
  };
  console.log(jsonData);
  // Enviar solicitud POST al servidor Flask
  fetch("/send_data", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify(jsonData),
  })
    .then((response) => response.json())
    .then((data) => console.log(data))
    .catch((error) => {
      console.error("Error:", error);
    });
}

const esp32_button = document.querySelector(".button-esp32-icon");
esp32_button.addEventListener("click", () => {
  fetch("/program_esp32", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ command: "run_script" }),
  })
    .then((response) => {
      if (!response.ok) {
        throw new Error("Network response was not ok");
      }
      console.log("Script execution started");
    })
    .catch((error) => {
      console.error("Error:", error);
      alert("Failed to run the script."); // Alerta al usuario si hay un error
    });
  console.log("ESP32 button pressed");
});

// Initialize menu var-som controls
function initialize_menu_var_som_controls() {
  const speaker_button = document.querySelector(".button-speaker-icon");
  speaker_button.addEventListener("click", () => {
    send_speaker_command("toggle");
    console.log("speaker button pressed");
  });

  const set_button = document.querySelector(".set-button");
  set_button.addEventListener("click", () => {
    let day = document.querySelector(".set-day").value;
    let month = document.querySelector(".set-month").value;
    let year = document.querySelector(".set-year").value;
    let hour = document.querySelector(".set-hour").value;
    let minute = document.querySelector(".set-minute").value;
    let seconds = document.querySelector(".set-seconds").value;

    fetch("/set_time", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: JSON.stringify({ day, month, year, hour, minute, seconds }),
    })
      .then((response) => response.json())
      .then((data) => {
        // Descomponer la hora actualizada
        let [date, time] = timeData.current_time.split(" ");
        let [day, month, year] = date.split("/");
        let [hour, minute, seconds] = time.split(":");

        // Actualizar elementos HTML con la nueva hora
        document.querySelector(".show-day").textContent = day;
        document.querySelector(".show-month").textContent = month;
        document.querySelector(".show-year").textContent = year;
        document.querySelector(".show-hour").textContent = hour;
        document.querySelector(".show-minute").textContent = minute;
        document.querySelector(".show-seconds").textContent = seconds;
      })
      .catch((error) => {
        console.error("Error:", error);
      });
  });
}

function send_speaker_command(command) {
  // Enviar solicitud POST al servidor Flask
  fetch("/speaker_command", {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ command: command }),
  })
    .then((response) => response.json())
    .then((data) => console.log(data))
    .catch((error) => {
      console.error("Error:", error);
    });
}

// Modificación de los controladores de eventos para forwardButton y backwardButton
document
  .getElementById("forwardButton")
  .addEventListener("mousedown", () => handleButtonPress(true));
document
  .getElementById("backwardButton")
  .addEventListener("mousedown", () => handleButtonPress(false));
document
  .getElementById("forwardButton")
  .addEventListener("mouseup", handleButtonRelease);
document
  .getElementById("backwardButton")
  .addEventListener("mouseup", handleButtonRelease);

// Función para manejar el evento mousedown
function handleButtonPress(direction) {
    send_actuator_data("motor", motorPower, motorEnabled, direction);
}

// // Función para manejar el evento mouseup
// function handleButtonPress(direction) {
//   if (motorEnabled) {
//     send_actuator_data("motor", motorPower, motorEnabled, direction);
//   }
// }

function handleButtonRelease() {
  // Send data with power and enable set to 0 after a short delay
  setTimeout(() => {
    send_actuator_data("motor", 0, false, false);
  }, 100);
}
function updateCurrentTime() {
  setInterval(() => {
    fetch("/get_time")
      .then((response) => response.json())
      .then((data) => {
        const currentTime = data.current_time;
        // Assuming the format of currentTime is 'dd/mm/yyyy hh:mm:ss'
        const [date, time] = currentTime.split(" ");
        const [day, month, year] = date.split("/");
        const [hour, minute, seconds] = time.split(":");

        document.querySelector(".show-day").textContent = day;
        document.querySelector(".show-month").textContent = month;
        document.querySelector(".show-year").textContent = year;
        document.querySelector(".show-hour").textContent = hour;
        document.querySelector(".show-minute").textContent = minute;
        document.querySelector(".show-seconds").textContent = seconds;
      })
      .catch((error) => console.error("Error:", error));
  }, 1000); // Updates every second
}

var is_block = true;
function toggleActuatorsMenu(checkboxChecked) {
  is_block = !checkboxChecked;
  const actuatorsMenu = document.querySelector(".group-actuators-control-menu");
  if (actuatorsMenu) {
    actuatorsMenu.style.opacity = checkboxChecked ? "1" : "0.5";
  }
}

// Export start_interactions for use in main.js
export { start_interactions, toggleActuatorsMenu };
