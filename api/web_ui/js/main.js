import { start_interactions } from './interactions.js';
import { load_instructions } from './load_instructions.js';
import { connect_sensor_data } from './esp32_data_handler.js';

function main() {
    document.addEventListener('DOMContentLoaded', function() {
        start_interactions();
        load_instructions();
    });

    window.onload = function() {
        connect_sensor_data();
    };
}

main();
