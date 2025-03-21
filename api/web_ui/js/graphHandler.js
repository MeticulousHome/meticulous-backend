document.addEventListener('DOMContentLoaded', function () {
    const socket = io.connect('/');
    let isGraphing = false;
    let startTime = Date.now();

    toggleButton = document.getElementById('togglePlotting')
    toggleButton.addEventListener('click', () => {
        isGraphing = !isGraphing
        toggleButton.innerText = (isGraphing ? "Stop" : "Start") + " Plotting"
    });

    document.getElementById('resetZoom').addEventListener('click', function () {
        window.thermistorGraph.resetZoom();
        window.actuatorGraph.resetZoom();
        window.adcGraph.resetZoom();
        window.miscGraph.resetZoom();
    });

    initGraph('thermistorGraph', ['barUp', 'barMiddleUp', 'barMiddleDown', 'barDown', 'external1', 'external2', 'tube', 'motor_temp']);
    initGraph('actuatorGraph', ['position', 'speed', 'power', 'current', 'bandHeater_power', 'bandHeater_current']);
    initGraph('adcGraph', ['adc0_rate', 'adc1_rate', 'adc2_rate', 'adc3_rate', 'pressureSensor_rate']);
    initGraph('miscGraph', ['pressureSensor_pressure', 'flowSensor_flow', 'loadcell_weight', 'display_temp', 'set_pressure', 'set_flow', 'set_power', 'set_position']);

    const therm_up = document.getElementById('therm_up');
    const therm_midUp = document.getElementById('therm_midUp');
    const therm_midDown = document.getElementById('therm_midDown');
    const therm_down = document.getElementById('therm_down');
    const therm_ext1 = document.getElementById('therm_external1');
    const therm_ext2 = document.getElementById('therm_external2');
    const therm_tube = document.getElementById('therm_tube');
    const therm_motor = document.getElementById('therm_motor');

    const motor_position = document.getElementById('motor_position');
    const motor_speed = document.getElementById('motor_speed');
    const motor_power = document.getElementById('motor_power');
    const motor_current = document.getElementById('motor_current');
    const bandHeater_current = document.getElementById('bandHeater_current');

    const adc0_rate = document.getElementById('adc0_rate');
    const adc1_rate = document.getElementById('adc1_rate');
    const adc2_rate = document.getElementById('adc2_rate');
    const adc3_rate = document.getElementById('adc3_rate');
    const pressureSensor_rate = document.getElementById('pressureSensor_rate');


    const misc_pressure = document.getElementById('misc_pressure');
    const misc_temperature = document.getElementById('misc_temperature');
    const misc_flow = document.getElementById('misc_flow');
    const misc_weight = document.getElementById('misc_weight');


    const misc_name = document.getElementById('misc_name');
    const misc_time = document.getElementById('misc_time');
    const misc_profile = document.getElementById('misc_profile');
    const misc_state = document.getElementById('misc_state');
    const misc_extracting = document.getElementById('misc_extracting');

    const misc_set_pressure = document.getElementById('misc_set_pressure');
    const misc_set_flow = document.getElementById('misc_set_flow');
    const misc_set_power = document.getElementById('misc_set_power');
    const misc_set_position = document.getElementById('misc_set_position');
    const misc_limiter_active = document.getElementById('misc_limiter_active');


    window.thermistorData = {
        labels: [],
        data: {
            barUp: [],
            barMiddleUp: [],
            barMiddleDown: [],
            barDown: [],
            external1: [],
            external2: [],
            tube: [],
            motorTemp: []
        }
    };

    window.actuatorData = {
        labels: [],
        data: {
            position: [],
            speed: [],
            power: [],
            current: [],
            bandHeater_power: [],
            bandHeater_current: [],
        }
    };

    window.adcData = {
        labels: [],
        data: {
            adc0_rate: [],
            adc1_rate: [],
            adc2_rate: [],
            adc3_rate: [],
            pressureSensor_rate: []
        }
    };

    window.miscData = {
        labels: [],
        data: {
            pressureSensor_pressure: [],
            flowSensor_flow: [],
            loadcell_weight: [],
            display_temp: [],
            set_pressure: [],
            set_flow: [],
            set_power: [],
            set_position: [],
        }
    };

    socket.on("status", function (data) {
        const currentTime = (Date.now() - startTime) / 1000;
        // Unsused fields: 
        // - data.name (status)
        // - data.time (shot time)
        // - data.profile (profile name)
        updateGraphData(window.miscData, currentTime, {
            pressureSensor_pressure: data.sensors.p,
            flowSensor_flow: data.sensors.f,
            loadcell_weight: data.sensors.w,
            display_temp: data.sensors.t,
            set_pressure: data.setpoints?.pressure,
            set_flow: data.setpoints?.flow,
            set_power: data.setpoints?.power,
            set_position: data.setpoints?.position,
        });
        misc_pressure.innerText = data.sensors.p;
        misc_flow.innerText = data.sensors.f;
        misc_temperature.innerText = data.sensors.t;
        misc_weight.innerText = data.sensors.w;

        misc_name.innerText = data.name;
        misc_time.innerText = data.time;
        misc_profile.innerText = data.profile;
        misc_state.innerText = data.state;
        misc_extracting.innerText = data.extracting;

        setpoint = data.setpoints;

        if (setpoint?.pressure) {
            misc_set_pressure.innerText = setpoint.pressure;
        } else {
            misc_set_pressure.innerText = "N/A";
        }

        if (setpoint?.flow) {
            misc_set_flow.innerText = setpoint.flow;
        } else {
            misc_set_flow.innerText = "N/A";
        }

        if (setpoint?.power) {
            misc_set_power.innerText = setpoint.power;
        } else {
            misc_set_power.innerText = "N/A";
        }

        if (setpoint?.position) {
            misc_set_position.innerText = setpoint.position;
        } else {
            misc_set_position.innerText = "N/A";
        }

        if (setpoint?.limiter_active) {
            misc_limiter_active.innerText = data.limiter_active;
        } else {
            misc_limiter_active.innerText = "N/A";
        }

        if (isGraphing) {
            updateGraph('miscGraph', window.miscData);
        }
    });


    socket.on("sensors", function (data) {
        const currentTime = (Date.now() - startTime) / 1000;
        updateGraphData(window.thermistorData, currentTime, {
            barUp: data.t_bar_up,
            barMiddleUp: data.t_bar_mu,
            barMiddleDown: data.t_bar_md,
            barDown: data.t_bar_down,
            external1: data.t_ext_1,
            external2: data.t_ext_2,
            tube: data.t_tube,
            motorTemp: data.t_motor_temp
        })

        therm_up.innerText = data.t_bar_up;
        therm_midUp.innerText = data.t_bar_mu;
        therm_midDown.innerText = data.t_bar_md;
        therm_down.innerText = data.t_bar_down;
        therm_ext1.innerText = data.t_ext_1;
        therm_ext2.innerText = data.t_ext_2;
        therm_tube.innerText = data.t_tube;
        therm_motor.innerText = data.t_motor_temp;

        if (isGraphing) {
            updateGraph('thermistorGraph', window.thermistorData);
        }

    })

    socket.on("comunication", function (data) {
        const currentTime = (Date.now() - startTime) / 1000;
        new_data = {
            adc_devices: [
                { adc0: { "rate": data.a_0 } },
                { adc1: { "rate": data.a_1 } },
                { adc2: { "rate": data.a_2 } },
                { adc3: { "rate": data.a_3 } },
            ],
            pressureSensor: { "rate": data.p }
        }

        adc0_rate.innerText = data.a_0;
        adc1_rate.innerText = data.a_1;
        adc2_rate.innerText = data.a_2;
        adc3_rate.innerText = data.a_3;
        pressureSensor_rate.innerText = data.p;

        updateGraphData(window.adcData, currentTime, extractAdcRates(new_data));
        if (isGraphing) {
            updateGraph('adcGraph', window.adcData);
        }
    })

    socket.on("actuators", function (data) {
        const currentTime = (Date.now() - startTime) / 1000;

        updateGraphData(window.actuatorData, currentTime, {
            position: data.m_pos,
            speed: data.m_spd,
            power: data.m_pwr,
            current: data.m_cur,
            bandHeater_power: data.bh_pwr,
            bandHeater_current: data.bh_cur,
        })

        motor_position.innerText = data.m_pos;
        motor_current.innerText = data.m_cur;
        motor_speed.innerText = data.m_spd;
        motor_power.innerText = data.m_pwr;
        bandHeater_power.innerText = data.bh_pwr;
        bandHeater_current.innerText = data.bh_cur;

        if (isGraphing) {
            updateGraph('actuatorGraph', window.actuatorData);
        }

    })


    socket.on("meticulous_data", function (data) {
        if (isGraphing) {
            const currentTime = (Date.now() - startTime) / 1000;
            updateGraphData(window.thermistorData, currentTime, data.thermistor);
            updateGraph('thermistorGraph', window.thermistorData);
            updateGraphData(window.actuatorData, currentTime, data.motor);
            updateGraph('actuatorGraph', window.actuatorData);
            updateGraphData(window.adcData, currentTime, extractAdcRates(data.adc_devices));
            updateGraph('adcGraph', window.adcData);
            updateGraphData(window.miscData, currentTime, {
                bandHeater_power: data.bandHeater.power,
                pressureSensor_pressure: data.pressureSensor.pressure,
                flowSensor_flow: data.flowSensor.flow,
                loadcell_weight: data.loadcell.weight
            });
            updateGraph('miscGraph', window.miscData);
        }
    });


});

function extractAdcRates(adcDevices) {
    let rates = {};
    adcDevices.adc_devices.forEach((device, index) => {
        rates[`adc${index}_rate`] = device[`adc${index}`]?.rate || 0;
    });
    rates['pressureSensor_rate'] = adcDevices.pressureSensor?.rate || 0;
    return rates;
}

function initGraph(canvasId, datasetKeys) {
    const ctx = document.getElementById(canvasId).getContext('2d');
    let yScalesOptions;

    switch (canvasId) {
        case 'thermistorGraph':
            yScalesOptions = {
                suggestedMin: 80,
                suggestedMax: 100,
            };
            break;
        case 'actuatorGraph':
            yScalesOptions = {
                suggestedMin: -100,
                suggestedMax: 100,
            };
            break;
        case 'adcGraph':
            yScalesOptions = {
                suggestedMin: 0,
                suggestedMax: 150,
            };
            break;
        case 'miscGraph':
            yScalesOptions = {
                suggestedMin: 20,
                suggestedMax: 100,
            };
            break;
        default:
            yScalesOptions = {
                beginAtZero: false,
            };
    }

    window[canvasId] = new Chart(ctx, {
        type: 'line',
        data: {
            labels: [],
            datasets: datasetKeys.map(key => ({
                label: key,
                data: [],
                borderColor: randomColor(key),
                fill: false,
                pointRadius: 0,
                lineTension: 0
            }))
        },
        options: {
            spanGaps: false,
            scales: {
                y: Object.assign({}, yScalesOptions, {
                    ticks: {
                        color: '#555555'
                    },
                    grid: {
                        color: '#555555'
                    }
                }),
                x: {
                    ticks: {
                        color: '#555555',
                        maxTicksLimit: 10
                    },
                    grid: {
                        display: false,
                        drawBorder: false
                    }
                }
            },
            legend: {
                labels: {
                    fontColor: '#FFFFFF',
                    fontSize: 16
                }
            },
            tooltips: {
                mode: 'index',
                intersect: false,
                titleFontColor: '#FFFFFF',
                bodyFontColor: '#FFFFFF'
            },
            plugins: {
                zoom: {
                    pan: {
                        enabled: true,
                        mode: 'xy',
                        threshold: 10,
                        modifierKey: 'ctrl'
                    },
                    zoom: {
                        wheel: {
                            enabled: true
                        },
                        drag: {
                            enabled: true,
                            mode: 'xy'
                        },
                        pinch: {
                            enabled: true,
                            mode: 'xy'
                        }
                    }
                }
            },
            animation: {
                duration: 0
            }
        }
    });
}


function updateGraph(canvasId, graphData) {
    const chart = window[canvasId];
    if (chart) {
        chart.data.labels = graphData.labels;
        chart.data.datasets.forEach((dataset, index) => {
            dataset.data = graphData.data[Object.keys(graphData.data)[index]];
        });
        chart.update('none');
    }
}


function updateGraphData(graphData, currentTime, newData) {
    graphData.labels.push(currentTime.toString());
    Object.keys(graphData.data).forEach(key => {
        let value = newData[key];
        if (value !== undefined) {
            graphData.data[key].push(parseFloat(value) || 0);
        } else {
            graphData.data[key].push(0);
        }
    });

    if (graphData.labels.length > 1000) {
        graphData.labels.shift();
        Object.keys(graphData.data).forEach(key => {
            graphData.data[key].shift();
        });
    }
}

const yellowColor = '#FFE524'; // Yellow
const orangeLightColor = '#E89F00'; // Orange Light
const orangeDeeperColor = '#C76300'; // Orange Deeper
const orangeDarkColor = '#E87C00'; // Orange Dark
const redColor = '#E4321b'; // Red
const burgundyColor = '#89023E'; // Burgundy
const blueColor = '#3C59AB'; // Blue
const tealColor = '#007A8A'; // Teal
const whiteColor = '#FFFFFF'; //White

function randomColor(key) {
    const colorMapping = {
        'position': yellowColor,
        'speed': orangeDarkColor,
        'power': redColor,
        'current': blueColor,
        'bandHeater_power': tealColor,

        // ADC values
        'adc0_rate': yellowColor,
        'adc1_rate': orangeDarkColor,
        'adc2_rate': redColor,
        'adc3_rate': blueColor,
        'pressureSensor_rate': tealColor,

        // Misc graph
        'pressureSensor_pressure': yellowColor,
        'flowSensor_flow': redColor,
        'loadcell_weight': blueColor,
        'display_temp': tealColor,

        // Temperature Graph
        'barUp': yellowColor,
        'barMiddleUp': orangeLightColor,
        'barMiddleDown': orangeDarkColor,
        'barDown': orangeDeeperColor,
        'external1': redColor,
        'external2': burgundyColor,
        'tube': tealColor,
        'motorTemp': blueColor,
        'bandHeater_current': whiteColor,
    };

    return colorMapping[key] || (console.log("No color was found for " + key) || '#FFFFFF');
}
