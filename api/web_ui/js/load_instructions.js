import { toggleActuatorsMenu } from "./interactions.js";

function save_checkbox_state(checkbox) {
  localStorage.setItem(checkbox.id, checkbox.checked);
}

function restore_checkbox_state(checkbox) {
  const is_checked = localStorage.getItem(checkbox.id) === 'true';
  checkbox.checked = is_checked;
  if (checkbox.id === "test0-checkbox") {
    toggleActuatorsMenu(is_checked);
  }
}

function clear_checkbox_states() {
  for (let key in localStorage) {
    if (key.startsWith('test')) {
      localStorage.removeItem(key);
    }
  }
}


function load_instructions() {
  document.querySelectorAll(".info").forEach((item) => {
    item.addEventListener("click", function (event) {
      let component_name = event.currentTarget.id.replace("info_", "");
      let file_name = `test_${component_name}.txt`;

      console.log("Clicked on info:", file_name);

      fetch(`static/js/instructions/${file_name}`)
        .then((response) => response.text())
        .then((data) => {
          const instructions_container = document.querySelector(".instructions");
          instructions_container.innerHTML = data;

          // Check if the loaded file is test_band_heater.txt
          if (file_name === "test_band_heater.txt") {
            // Handle the specific checkbox of test_band_heater.txt
            const test0_checkbox = document.getElementById("test0-checkbox");
            if (test0_checkbox) {
              restore_checkbox_state(test0_checkbox);
              test0_checkbox.addEventListener("change", () => {
                toggleActuatorsMenu(test0_checkbox.checked);
                save_checkbox_state(test0_checkbox);
              });
              // Check the initial state of the checkbox
              toggleActuatorsMenu(test0_checkbox.checked);
            }
          } else {
            // For other files, ensure the actuators menu is disabled
            toggleActuatorsMenu(false);
          }

          // Restore the state of each checkbox after loading the content
          instructions_container.querySelectorAll('input[type=checkbox]').forEach((checkbox) => {
            // Skip the band heater checkbox as it's already handled above
            if (checkbox.id !== "test0-checkbox") {
              restore_checkbox_state(checkbox);
              // Add an event listener to save the state when the checkbox changes
              checkbox.addEventListener('change', () => save_checkbox_state(checkbox));
            }
          });

        })
        .catch((error) => console.error("Error:", error));
    });
  });
}

// Export the function for use in main.js
export { load_instructions };

if (performance.getEntriesByType("navigation")[0].type === 'reload') {
  clear_checkbox_states();
}