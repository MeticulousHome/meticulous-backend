//! `hostname.py`: the machine's two-word identity ("aromatic" + "Espresso"),
//! the names derived from it, and setting the OS hostname.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::config::{ConfigStore, keys};
use crate::py::{CommandRunner, PyResult, argv, py_str};

pub struct HostnameManager {
    config: Arc<ConfigStore>,
    runner: parking_lot::RwLock<Arc<dyn CommandRunner>>,
}

impl HostnameManager {
    pub fn new(config: Arc<ConfigStore>, runner: Arc<dyn CommandRunner>) -> HostnameManager {
        HostnameManager {
            config,
            runner: parking_lot::RwLock::new(runner),
        }
    }

    #[cfg(test)]
    pub fn set_runner(&self, runner: Arc<dyn CommandRunner>) {
        *self.runner.write() = runner;
    }

    fn runner(&self) -> Arc<dyn CommandRunner> {
        self.runner.read().clone()
    }

    /// `HostnameManager.init()`: draw the identifier pair once per machine.
    pub fn init(&self) {
        if identifier_len(&self.config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER)) == 2 {
            return;
        }
        let (adjective, noun) = random_identifier();
        tracing::info!("Created new device identifier pair:");
        self.config.set(
            keys::SYSTEM,
            keys::DEVICE_IDENTIFIER,
            json!([adjective, noun]),
        );
        self.config.save();
    }

    /// `getMachineIdentifierCamelCase()`.
    pub fn identifier_camel_case(&self) -> Option<String> {
        let ident = self.config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER);
        let parts = ident.as_array().filter(|a| a.len() == 2)?;
        Some(format!(
            "{}{}",
            capitalize(&py_str(&parts[0])),
            py_str(&parts[1])
        ))
    }

    /// `generateDeviceName()`: the BLE name, the access point SSID and the
    /// name the apps show.
    pub fn generate_device_name(&self) -> String {
        let ident = self
            .identifier_camel_case()
            .unwrap_or_else(|| "Espresso".into());
        format!("Meticulous{ident}")
    }

    /// `generateHostname()`.
    pub fn generate_hostname(&self) -> String {
        let ident = self
            .identifier_camel_case()
            .unwrap_or_else(|| "Espresso".into());
        // PARITY: a machine without a serial number is "...-None".
        let serial = py_str(&self.config.get(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER));
        format!("meticulous{ident}-{serial}")
    }

    /// `setHostname(new_hostname)`. A failing `hostnamectl` is logged; a
    /// missing one raises, as `subprocess.run(..., check=True)` does.
    pub fn set_hostname(&self, new_hostname: &str) -> PyResult<()> {
        let runner = self.runner();
        let result = runner
            .run(&argv(&["hostnamectl", "set-hostname", new_hostname]), None)
            .map_err(|e| e.to_py())?;
        if result.returncode != 0 {
            tracing::error!("Failed to set hostname (CalledProcessError)");
            return Ok(());
        }
        tracing::info!("Hostname set to {new_hostname}");
        runner
            .run(
                &argv(&["systemctl", "restart", "rauc-hawkbit-updater"]),
                None,
            )
            .map_err(|e| e.to_py())?;
        tracing::info!("Restarted rauc-hawkbit-updater");
        Ok(())
    }
}

/// `len(MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER])`.
fn identifier_len(value: &Value) -> usize {
    match value {
        Value::Array(items) => items.len(),
        Value::String(s) => s.chars().count(),
        Value::Object(map) => map.len(),
        _ => 0,
    }
}

/// `_generateRandomIdentifierComponents()`: two random bytes index the lists.
fn random_identifier() -> (&'static str, &'static str) {
    let byte1 = rand::random_range(0usize..=0xFF);
    let byte2 = rand::random_range(0usize..=0xFF);
    (
        ADJECTIVES[byte1 % ADJECTIVES.len()],
        NOUNS[byte2 % NOUNS.len()],
    )
}

/// Python's `str.capitalize()`: the first character upper case, the rest
/// lower case ("fullBodied" becomes "Fullbodied").
pub fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        Some(first) => first
            .to_uppercase()
            .chain(chars.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

pub const ADJECTIVES: [&str; 246] = [
    "aromatic",
    "bitter",
    "smooth",
    "rich",
    "strong",
    "dark",
    "creamy",
    "bold",
    "fullBodied",
    "spicy",
    "nutty",
    "sweet",
    "roasted",
    "velvety",
    "balanced",
    "warm",
    "frothy",
    "intense",
    "earthy",
    "hearty",
    "subtle",
    "mild",
    "luscious",
    "toasted",
    "silky",
    "decadent",
    "caramelized",
    "dense",
    "flavorful",
    "invigorating",
    "gourmet",
    "buttery",
    "mellow",
    "tantalizing",
    "acidic",
    "exotic",
    "robust",
    "sumptuous",
    "zesty",
    "syrupy",
    "handcrafted",
    "thick",
    "foamy",
    "artisanal",
    "complex",
    "delectable",
    "steaming",
    "heady",
    "indulgent",
    "brewed",
    "luxurious",
    "charred",
    "crisp",
    "organic",
    "refreshing",
    "intoxicating",
    "tempting",
    "piping",
    "filtered",
    "layered",
    "diverse",
    "energizing",
    "wholesome",
    "satisfying",
    "traditional",
    "ground",
    "delicate",
    "unique",
    "fresh",
    "homemade",
    "infused",
    "local",
    "golden",
    "exquisite",
    "premium",
    "aged",
    "festive",
    "soothing",
    "contemporary",
    "european",
    "chocolaty",
    "dreamy",
    "seasonal",
    "special",
    "cozy",
    "handmade",
    "custom",
    "honeyed",
    "refined",
    "rustic",
    "authentic",
    "steamed",
    "fragrant",
    "whipped",
    "energetic",
    "casual",
    "light",
    "zingy",
    "smoky",
    "lively",
    "vibrant",
    "exuberant",
    "exhilarating",
    "inspiring",
    "brisk",
    "edgy",
    "trendy",
    "artful",
    "imaginative",
    "captivating",
    "dashing",
    "peppy",
    "perky",
    "lustrous",
    "magical",
    "enchanting",
    "euphoric",
    "jubilant",
    "blissful",
    "serene",
    "tranquil",
    "graceful",
    "eloquent",
    "sophisticated",
    "chic",
    "elegant",
    "fashionable",
    "stylish",
    "swanky",
    "ritzy",
    "plush",
    "lavish",
    "posh",
    "deluxe",
    "opulent",
    "grand",
    "magnificent",
    "sumptuous",
    "luxurious",
    "classy",
    "swish",
    "splendid",
    "majestic",
    "regal",
    "stately",
    "august",
    "imposing",
    "impressive",
    "monumental",
    "grandiose",
    "heroic",
    "epic",
    "legendary",
    "iconic",
    "celebrated",
    "renowned",
    "famous",
    "notable",
    "eminent",
    "distinguished",
    "prestigious",
    "acclaimed",
    "esteemed",
    "respected",
    "honored",
    "venerated",
    "revered",
    "admired",
    "cherished",
    "beloved",
    "valued",
    "prized",
    "treasured",
    "appreciated",
    "recognized",
    "acknowledged",
    "commended",
    "praised",
    "lauded",
    "applauded",
    "celebrated",
    "extolled",
    "glorified",
    "exalted",
    "deified",
    "idolized",
    "worshipped",
    "reverenced",
    "venerated",
    "hallowed",
    "sanctified",
    "blessed",
    "consecrated",
    "anointed",
    "canonized",
    "beatified",
    "exemplary",
    "paragon",
    "model",
    "archetype",
    "epitome",
    "quintessence",
    "personification",
    "embodiment",
    "incarnation",
    "manifestation",
    "avatar",
    "prodigy",
    "virtuoso",
    "maestro",
    "master",
    "guru",
    "sage",
    "wizard",
    "magician",
    "enchanter",
    "sorcerer",
    "necromancer",
    "alchemist",
    "conjurer",
    "warlock",
    "witch",
    "seer",
    "prophet",
    "oracle",
    "mystic",
    "clairvoyant",
    "psychic",
    "medium",
    "shaman",
    "healer",
    "therapist",
    "counselor",
    "guide",
    "mentor",
    "teacher",
    "instructor",
    "trainer",
    "coach",
    "tutor",
    "educator",
    "lecturer",
    "preacher",
    "evangelist",
    "missionary",
    "crusader",
];

pub const NOUNS: [&str; 230] = [
    "Espresso",
    "Latte",
    "Cappuccino",
    "Mocha",
    "Americano",
    "Macchiato",
    "Ristretto",
    "Frappuccino",
    "Breve",
    "Cortado",
    "Doppio",
    "Affogato",
    "Lungo",
    "FlatWhite",
    "RedEye",
    "BlackCoffee",
    "DripCoffee",
    "FilterCoffee",
    "ColdBrew",
    "NitroCoffee",
    "IcedCoffee",
    "TurkishCoffee",
    "GreekCoffee",
    "IrishCoffee",
    "VienneseCoffee",
    "CafeAuLait",
    "CafeConLeche",
    "CafeCrema",
    "CafeCubano",
    "CafeZorro",
    "Galao",
    "CoffeeMilk",
    "EggnogLatte",
    "PumpkinSpiceLatte",
    "ChaiLatte",
    "CaramelMacchiato",
    "VanillaLatte",
    "MochaLatte",
    "HazelnutLatte",
    "ToffeeNutLatte",
    "MapleLatte",
    "GingerbreadLatte",
    "HoneyLatte",
    "SoyLatte",
    "AlmondMilkLatte",
    "OatMilkLatte",
    "RiceMilkLatte",
    "CoconutMilkLatte",
    "EspressoConPanna",
    "EspressoMacchiato",
    "EspressoTonic",
    "EspressoRomano",
    "EspressoMartini",
    "EspressoShot",
    "DecafEspresso",
    "Barista",
    "CoffeeGrinder",
    "CoffeeMaker",
    "EspressoMachine",
    "FrenchPress",
    "Aeropress",
    "MokaPot",
    "Percolator",
    "SiphonCoffeeMaker",
    "CoffeeRoaster",
    "CoffeeFilter",
    "CoffeeScoop",
    "Tamper",
    "MilkFrother",
    "SteamWand",
    "KnockBox",
    "Portafilter",
    "GroupHead",
    "CoffeeScale",
    "ShotGlass",
    "CappuccinoCup",
    "LatteMug",
    "EspressoCup",
    "Demitasse",
    "CoffeeMug",
    "TravelMug",
    "CoffeeThermos",
    "CoffeePot",
    "CoffeeCanister",
    "CoffeeBeans",
    "GroundCoffee",
    "CoffeeCapsule",
    "CoffeePod",
    "CoffeePuck",
    "CoffeeGrounds",
    "RoastedBeans",
    "GreenBeans",
    "SingleOrigin",
    "CoffeeBlend",
    "FairTradeCoffee",
    "OrganicCoffee",
    "ShadeGrownCoffee",
    "KonaCoffee",
    "ArabicaBeans",
    "RobustaBeans",
    "LightRoast",
    "MediumRoast",
    "DarkRoast",
    "FrenchRoast",
    "ItalianRoast",
    "EspressoRoast",
    "CoffeeCreamer",
    "HalfAndHalf",
    "WhippedCream",
    "Milk",
    "SoyMilk",
    "AlmondMilk",
    "OatMilk",
    "RiceMilk",
    "CoconutMilk",
    "Sugar",
    "BrownSugar",
    "Honey",
    "MapleSyrup",
    "AgaveNectar",
    "VanillaSyrup",
    "CaramelSyrup",
    "HazelnutSyrup",
    "AlmondSyrup",
    "ChocolateSyrup",
    "Cinnamon",
    "Nutmeg",
    "CocoaPowder",
    "ChocolatePowder",
    "WhippedTopping",
    "CoffeeStirrer",
    "CoffeeSpoon",
    "CoffeeStraw",
    "Napkin",
    "Coaster",
    "CoffeeTable",
    "BarStool",
    "CafeChair",
    "Sofa",
    "CoffeeShop",
    "Cafe",
    "EspressoBar",
    "CoffeeStand",
    "CoffeeKiosk",
    "CoffeeCart",
    "CoffeeTruck",
    "CoffeeFestival",
    "CoffeeTasting",
    "CoffeeWorkshop",
    "CoffeeCourse",
    "LatteArt",
    "CoffeeArt",
    "Cupping",
    "Brewing",
    "Grinding",
    "Tamping",
    "Frothing",
    "Steaming",
    "Pouring",
    "Serving",
    "Sip",
    "Guzzle",
    "Taste",
    "Savor",
    "Enjoy",
    "Crave",
    "Indulge",
    "Relax",
    "Unwind",
    "Socialize",
    "Chat",
    "Meet",
    "Gather",
    "Connect",
    "Network",
    "Caffeine",
    "Decaffeination",
    "Aroma",
    "Flavor",
    "Body",
    "Acidity",
    "Aftertaste",
    "Crema",
    "Foam",
    "Bloom",
    "Extraction",
    "OverExtraction",
    "UnderExtraction",
    "BrewRatio",
    "Dose",
    "Yield",
    "Temperature",
    "Pressure",
    "Consistency",
    "Quality",
    "Freshness",
    "Origin",
    "Terroir",
    "Altitude",
    "Harvest",
    "Processing",
    "Washing",
    "Drying",
    "Fermentation",
    "Pulping",
    "Hulling",
    "Sorting",
    "Grading",
    "CuppingScore",
    "FlavorProfile",
    "TastingNotes",
    "AromaWheel",
    "CoffeeCulture",
    "CoffeeHistory",
    "CoffeeTradition",
    "CoffeeCeremony",
    "CoffeeRitual",
    "CoffeeBreak",
    "CoffeeHour",
    "CoffeeDate",
    "CoffeeLover",
    "CoffeeEnthusiast",
    "CoffeeAficionado",
    "CoffeeConnoisseur",
    "CoffeeGeek",
    "CoffeeSnob",
    "CoffeeAddict",
    "CoffeeHabit",
    "CoffeeRoutine",
    "CoffeeExperience",
];

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::py::fake::RecordingRunner;
    use crate::py::{Completed, RunError};

    fn manager() -> (
        tempfile::TempDir,
        Arc<crate::app::App>,
        Arc<RecordingRunner>,
    ) {
        let (dir, app) = crate::testing::app();
        let runner = RecordingRunner::ok();
        app.hostname.set_runner(runner.clone());
        (dir, app, runner)
    }

    /// hostname.py `HostnameManager.init`: a pair is drawn once and kept.
    #[test]
    fn init_draws_the_identifier_once() {
        let (_dir, app, _runner) = manager();
        assert_eq!(
            app.config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER),
            json!([])
        );
        app.hostname.init();
        let ident = app.config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER);
        let pair = ident.as_array().expect("pair");
        assert_eq!(pair.len(), 2);
        assert!(ADJECTIVES.contains(&pair[0].as_str().expect("str")));
        assert!(NOUNS.contains(&pair[1].as_str().expect("str")));
        app.hostname.init();
        assert_eq!(app.config.get(keys::SYSTEM, keys::DEVICE_IDENTIFIER), ident);
    }

    /// The names derived from the pair; `str.capitalize()` lowers the rest.
    #[test]
    fn names_from_the_identifier() {
        let (_dir, app, _runner) = manager();
        assert_eq!(app.hostname.generate_device_name(), "MeticulousEspresso");
        // PARITY: no serial number gives "-None".
        assert_eq!(app.hostname.generate_hostname(), "meticulousEspresso-None");
        app.config.set(
            keys::SYSTEM,
            keys::DEVICE_IDENTIFIER,
            json!(["fullBodied", "FlatWhite"]),
        );
        app.config
            .set(keys::SYSTEM, keys::MACHINE_SERIAL_NUMBER, json!("007"));
        assert_eq!(
            app.hostname.generate_device_name(),
            "MeticulousFullbodiedFlatWhite"
        );
        assert_eq!(
            app.hostname.generate_hostname(),
            "meticulousFullbodiedFlatWhite-007"
        );
        assert_eq!(capitalize("aROMATIC"), "Aromatic");
        assert_eq!(capitalize(""), "");
    }

    /// `setHostname`: hostnamectl, then the updater restarts; a failing
    /// hostnamectl is logged, a missing one raises.
    #[test]
    fn set_hostname_commands() {
        let (_dir, app, runner) = manager();
        app.hostname.set_hostname("meticulousX-1").expect("set");
        assert_eq!(
            runner.calls(),
            vec![
                ("hostnamectl set-hostname meticulousX-1".to_string(), false),
                ("systemctl restart rauc-hawkbit-updater".to_string(), false),
            ]
        );

        let failing = RecordingRunner::new(|_, _| {
            Ok(Completed {
                returncode: 1,
                ..Completed::default()
            })
        });
        app.hostname.set_runner(failing.clone());
        app.hostname.set_hostname("x").expect("logged, not raised");
        assert_eq!(failing.calls().len(), 1);

        let missing = RecordingRunner::new(|_, _| Err(RunError::NotFound("hostnamectl".into())));
        app.hostname.set_runner(missing);
        let error = app.hostname.set_hostname("x").unwrap_err();
        assert_eq!(error.kind, "FileNotFoundError");
    }
}
