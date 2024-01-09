function connect_sensor_data() {
  console.log("Conectando a sensor_data...");
  const isDevelopment = window.location.hostname === 'localhost' || window.location.hostname === '127.0.0.1';
  const socketURL = isDevelopment ? 'http://localhost:8080' : 'http://192.168.50.10:5000';
  const socket = io.connect(socketURL);

  const esp32Button = document.querySelector(".button-esp32-icon");
  let hardResetReceived = false;

  esp32Button.addEventListener("click", function () {
    hardResetReceived = false;
    setProgrammingText("Programming ESP32 ...");
    handleTimeout();
  });
  // Escucha de datos de sensores
  socket.on("sensor_data", function (data) {
    console.log("Datos de sensor recibidos:", data);
    updateSensorData(data);
    updateI2CAddresses(data);
  });

  socket.on("meticulous_data", function (data) {
    updateSensorDataFromMeticulous(data)
  })

  socket.on("log_message", function (message) {
    // Encuentra el primer elemento <p> con la clase "usb"
    const usbElement = document.querySelector(".usb");
    if (usbElement) {
      // Actualiza el texto del elemento <p> con el mensaje
      usbElement.textContent = message;
    }
  });

  // Escucha de la salida del ESP32
  socket.on("esp32_output", function (messageObj) {
    const message = messageObj.data;
    console.log("ESP32 output:", message);

    if (message === "Hard resetting via RTS pin...") {
      hardResetReceived = true;
    }

    if (message.toLowerCase() === "finish") {
      if (hardResetReceived) {
        setProgrammingText("Successful", true);
      } else {
        setProgrammingText("An error occurred", true);
      }
      clearTimeout(timeoutId); // Limpiar el timeout
    }
  });
}

function updateSensorData(data) {
  setElementText("weight", data.loadcell.weight, 'gr');
  setElementText("encoder_position", data.motor.encoderPosition); 
  setElementText("pressure", data.pressureSensor.pressure, 'bar');
  setElementText("motor_current", data.motor.current, 'A');
  let ohmsChannel1 = (data.thermistor.external1 * 1000) / (3.3 - data.thermistor.external1);
  setElementText("adc1_chanel1", ohmsChannel1, "Ω");
  let ohmsChannel2 = (data.thermistor.external2 * 1000) / (3.3 - data.thermistor.external2);
  setElementText("adc1_chanel2", ohmsChannel2, "Ω");
  setElementText("frequency", data.inletPower.frequency, "Hz");
  setElementText("inlet_voltage", data.inletPower.voltage, "V");
  setElementText("heater_driver_current", data.heater_driver.current, "A");
  let i2cExpanderStatus = data.i2c_expander.available === "true" ? "Available" : "Not available";
  setElementText("i2c_expander_available", i2cExpanderStatus);
  setElementText("knob_position", data.knob.position);
  setElementText("knob_button", parseInt(data.knob.button) === 1 ? '0' : '1');
  setElementText("buttons_start", data.buttons.start ? '1' : '0');
  setElementText("buttons_tare", data.buttons.tare ? '1' : '0');
  setElementText("buttons_spare_1", data.buttons.spare1 ? '1' : '0');
  setElementText("buttons_spare_2", data.buttons.spare2 ? '1' : '0');
  setElementText("motor_speed", data.motor.speed, "mm/s");

  // Actualizaciones para ADC
  setElementText(
    "id_voltage-adc1-0",
    data.adc_devices.adc_devices[1].channel[0],
    "V"
  );
  setElementText(
    "id_voltage-adc1-1",
    data.adc_devices.adc_devices[1].channel[1],
    "V"
  );
  setElementText(
    "id_voltage-adc1-2",
    data.adc_devices.adc_devices[1].channel[2],
    "V"
  );
  setElementText(
    "id_voltage-adc1-3",
    data.adc_devices.adc_devices[1].channel[3],
    "V"
  );

  setElementText(
    "voltage-adc2-0",
    data.adc_devices.adc_devices[2].channel[0],
    "V"
  );
  setElementText(
    "voltage-adc2-1",
    data.adc_devices.adc_devices[2].channel[1],
    "V"
  );
  setElementText(
    "voltage-adc2-2",
    data.adc_devices.adc_devices[2].channel[2],
    "V"
  );
  setElementText(
    "voltage-adc2-3",
    data.adc_devices.adc_devices[2].channel[3],
    "V"
  );
  console.log("Datos del sensor actualizados correctamente.");
}

function updateSensorDataFromMeticulous(data) {
  setElementText("weight", data.loadcell ? data.loadcell.weight : 'none', 'gr');
  setElementText("encoder_position", 'none');
  setElementText("pressure", data.pressureSensor ? data.pressureSensor.pressure : 'none', 'bar');
  setElementText("motor_current", data.motor ? data.motor.current : 'none', 'A');
  setElementText("motor_speed", data.motor ? data.motor.speed : 'none', "mm/s");

  setElementText("adc1_chanel1", data.thermistor && data.thermistor.external1 ? data.thermistor.external1 : 'none', "°C");
  setElementText("adc1_chanel2", data.thermistor && data.thermistor.external2 ? data.thermistor.external2 : 'none', "°C");

  setElementText("frequency", 'none', "Hz");
  setElementText("inlet_voltage", 'none', "V");
  setElementText("heater_driver_current", 'none', "A");

  setElementText("i2c_expander_available", 'none');
  setElementText("knob_position", 'none');
  setElementText("knob_button", 'none');
  setElementText("buttons_start", 'none');
  setElementText("buttons_tare", 'none');
  setElementText("buttons_spare_1", 'none');
  setElementText("buttons_spare_2", 'none');


  setElementText("id_voltage-adc1-0", 'none', "V");
  setElementText("id_voltage-adc1-1", 'none', "V");
  setElementText("id_voltage-adc1-2", 'none', "V");
  setElementText("id_voltage-adc1-3", 'none', "V");
  setElementText("voltage-adc2-0", 'none', "V");
  setElementText("voltage-adc2-1", 'none', "V");
  setElementText("voltage-adc2-2", 'none', "V");
  setElementText("voltage-adc2-3", 'none', "V");


  setElementText("bar_up", data.thermistor && data.thermistor.barUp ? data.thermistor.barUp : 'none');
  setElementText("bar_middle_up", data.thermistor && data.thermistor.barMiddleUp ? data.thermistor.barMiddleUp : 'none');
  setElementText("bar_middle_down", data.thermistor && data.thermistor.barMiddleDown ? data.thermistor.barMiddleDown : 'none');
  setElementText("bar_down", data.thermistor && data.thermistor.barDown ? data.thermistor.barDown : 'none');
  setElementText("flow", data.flowSensor ? data.flowSensor.flow : 'none');
  setElementText("band_heater_power", data.bandHeater ? data.bandHeater.power : 'none');
  setElementText("thermistor_valv", data.thermistor && data.thermistor.valv ? data.thermistor.valv : 'none');
  setElementText("thermistor_tube", data.thermistor && data.thermistor.tube ? data.thermistor.tube : 'none');
  setElementText("motor_position", data.motor ? data.motor.position : 'none');
  setElementText("motor_power", data.motor ? data.motor.power : 'none');
}


function updateI2CAddresses(data) {
  let i2cBus1Addresses = data.i2c.i2c[0].addresses;
  let i2cBus2Addresses = data.i2c.i2c[1].addresses;

  let bus1HTML = generateHTMLForAddresses(i2cBus1Addresses, "Bus 1 Addresses");
  let bus2HTML = generateHTMLForAddresses(i2cBus2Addresses, "Bus 2 Addresses");

  document.querySelector(".print-addresses").innerHTML = `<div class="addresses-container">${bus1HTML}${bus2HTML}</div>`;
}

function generateHTMLForAddresses(addresses, title, isOrdered = false) {
  if (typeof addresses === 'string' || !Array.isArray(addresses)) {
    console.error('Expected an array or a string for addresses, got:', addresses);
    return '';
  }

  let listItems = addresses.map(address => `<li>0x${address}</li>`).join("");
  let listType = isOrdered ? "ol" : "ul";
  return `<div class="address-column"><h3>${title}:</h3><${listType}>${listItems}</${listType}></div>`;
}

function setElementText(elementId, text, unit = "") {
  let element = document.getElementById(elementId);
  if (element) {
    let formattedText = text;
    if (typeof text === "number") {
      formattedText = text.toFixed(2) + unit;
    }
    element.textContent = formattedText;
  } else {
    console.warn(`Element with ID '${elementId}' not found.`);
  }
}


// Establecer el texto de programación
function setProgrammingText(text, resetAfterDelay = false) {
  const programmingText = document.querySelector(".press-the-icon1");
  programmingText.textContent = text;

  if (resetAfterDelay) {
    setTimeout(() => {
      programmingText.textContent = "Press the icon to program the ESP32";
    }, 30000);
  }
}

// Manejar timeout
let timeoutId;
function handleTimeout() {
  clearTimeout(timeoutId);
  timeoutId = setTimeout(() => {
    setProgrammingText("An error occurred", true);
  }, 120000); // 2 minutos
}

// Exportación de la función principal
export { connect_sensor_data };
