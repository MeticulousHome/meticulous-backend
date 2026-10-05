import subprocess
import random
import socket

from config import (
    CONFIG_SYSTEM,
    CONFIG_USER,
    DEVICE_IDENTIFIER,
    HOSTNAME_OVERRIDE,
    MACHINE_SERIAL_NUMBER,
    MeticulousConfig,
)
from log import MeticulousLogger

logger = MeticulousLogger.getLogger(__name__)


class HostnameManager:

    @staticmethod
    def _configuredIdentifierIsValid() -> bool:
        identifier = MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER]
        return (
            isinstance(identifier, list)
            and len(identifier) == 2
            and all(isinstance(component, str) and component for component in identifier)
        )

    @staticmethod
    def identifierFromHostname(hostname: str) -> tuple[str, str] | None:
        """Recover a generated machine identifier from an established hostname."""
        serial = MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]
        if not isinstance(hostname, str) or not hostname or serial is None:
            return None

        short_hostname = hostname.split(".", 1)[0]
        prefix = "meticulous"
        suffix = f"-{serial}"
        if not short_hostname.startswith(prefix) or not short_hostname.endswith(suffix):
            return None

        encoded_identifier = short_hostname[len(prefix) : -len(suffix)]
        for adjective in HostnameManager.ADJECTIVES:
            adjective_component = adjective.capitalize()
            if not encoded_identifier.casefold().startswith(adjective_component.casefold()):
                continue
            noun_component = encoded_identifier[len(adjective_component) :]
            for noun in HostnameManager.NOUNS:
                if noun_component.casefold() == noun.casefold():
                    return (adjective, noun)
        return None

    # We are using random identifier here instead of deriving it from something
    def _generateRandomIdentifierComponents() -> tuple[str, str]:
        byte1 = random.randint(0x00, 0xFF)
        byte2 = random.randint(0x00, 0xFF)

        # Use these bytes to select an adjective and a noun
        adjective = HostnameManager.ADJECTIVES[byte1 % len(HostnameManager.ADJECTIVES)]
        noun = HostnameManager.NOUNS[byte2 % len(HostnameManager.NOUNS)]
        return (adjective, noun)

    def init():
        recovered_identifier = HostnameManager.identifierFromHostname(socket.gethostname())
        hostname_override = MeticulousConfig[CONFIG_USER][HOSTNAME_OVERRIDE]
        configured_identifier = MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER]

        # A recognizable deployed hostname is the safest canonical source when no
        # explicit override exists. This repairs both missing and conflicting config
        # without changing the customer-visible OS hostname.
        if hostname_override is None and recovered_identifier is not None:
            recovered_list = list(recovered_identifier)
            if configured_identifier != recovered_list:
                logger.warning("Recovered device identifier from established hostname")
                MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER] = recovered_list
                MeticulousConfig.save()
            return

        if HostnameManager._configuredIdentifierIsValid():
            return

        if recovered_identifier is not None:
            logger.warning("Recovered missing device identifier from established hostname")
            MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER] = list(recovered_identifier)
            MeticulousConfig.save()
            return

        adjective, noun = HostnameManager._generateRandomIdentifierComponents()
        logger.info("Created new device identifier pair")
        MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER] = [adjective, noun]
        MeticulousConfig.save()

    def getMachineIdentifierCamelCase() -> str:
        ident = MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER]
        if len(ident) != 2:
            return None
        return f"{ident[0].capitalize()}{ident[1]}"

    def getMachineIdentifierLowerCase() -> str:
        ident = MeticulousConfig[CONFIG_SYSTEM][DEVICE_IDENTIFIER]
        if len(ident) != 2:
            return None
        return f"{ident[0].lower()}{ident[1].lower()}"

    def generateDeviceName() -> str:
        ident = HostnameManager.getMachineIdentifierCamelCase()
        if ident is None:
            ident = "Espresso"
        return f"Meticulous{ident}"

    def generateHostname() -> str:
        ident = HostnameManager.getMachineIdentifierCamelCase()
        if ident is None:
            ident = "Espresso"
        return f"meticulous{ident}-{MeticulousConfig[CONFIG_SYSTEM][MACHINE_SERIAL_NUMBER]}"

    def setHostname(new_hostname):
        try:
            subprocess.run(["hostnamectl", "set-hostname", new_hostname], check=True)
            logger.info(f"Hostname set to {new_hostname}")
            subprocess.run(["systemctl", "restart", "rauc-hawkbit-updater"])
            logger.info("Restarted rauc-hawkbit-updater")
        except subprocess.CalledProcessError as e:
            logger.error(f"Failed to set hostname ({type(e).__name__})")

    ADJECTIVES = [
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
    ]

    NOUNS = [
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
    ]
