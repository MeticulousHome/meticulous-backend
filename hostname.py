import subprocess

from log import MeticulousLogger
logger = MeticulousLogger.getLogger(__name__)
class HostnameManager():

    def setHostname(new_hostname):
        try:
            subprocess.run(['hostnamectl', 'set-hostname', new_hostname], check=True)
            logger.info(f"Hostname set to {new_hostname}")
        except subprocess.CalledProcessError as e:
            logger.warn(f"Failed to set hostname: {e}")

    def generateHostnameComponents(mac_address: str) -> tuple[str, str]:
        # Extract last two bytes and convert them to integers
        bytes_hex = mac_address.split(':')[-2:]
        byte1 = int(bytes_hex[0], 16)
        byte2 = int(bytes_hex[1], 16)

        # Use these bytes to select an adjective and a noun
        adjective = HostnameManager.ADJECTIVES[byte1 % len(
            HostnameManager.ADJECTIVES)]
        noun = HostnameManager.NOUNS[byte2 % len(HostnameManager.NOUNS)]
        return (adjective, noun)

    def generateDeviceName(mac_address: str) -> str:
        (adjective, noun)  = HostnameManager.generateHostnameComponents(mac_address)
        adjective = adjective.title()
        return f"Meticulous{adjective}{noun}"

    def generateHostname(mac_address: str) -> str:
        (adjective, noun) = HostnameManager.generateHostnameComponents(mac_address)
        # Combine to form a hostname
        return f"meticulous-{adjective}{noun}"

    def checkAndUpdateHostname(current_hostname, mac_address: str):
        hostname = HostnameManager.generateHostname(mac_address)
        if current_hostname == hostname:
            return
        logger.info(f"Changing hostname new = {hostname}")
        HostnameManager.setHostname(hostname)

    ADJECTIVES = [
        "aromatic", "bitter", "smooth", "rich", "strong", "dark", "creamy", "bold",
        "fullBodied", "spicy", "nutty", "sweet", "roasted", "velvety", "balanced",
        "warm", "frothy", "intense", "earthy", "hearty", "subtle", "mild", "luscious",
        "toasted", "silky", "decadent", "caramelized", "dense", "flavorful", "invigorating",
        "gourmet", "buttery", "mellow", "tantalizing", "acidic", "exotic", "robust",
        "sumptuous", "zesty", "syrupy", "handcrafted", "thick", "foamy", "artisanal",
        "complex", "delectable", "steaming", "heady", "indulgent", "brewed", "luxurious",
        "charred", "crisp", "organic", "refreshing", "intoxicating", "tempting", "piping",
        "filtered", "layered", "diverse", "energizing", "wholesome", "satisfying", "traditional",
        "ground", "delicate", "unique", "fresh", "homemade", "infused", "local", "golden",
        "exquisite", "premium", "aged", "festive", "soothing", "contemporary", "european",
        "chocolaty", "dreamy", "seasonal", "special", "cozy", "handmade", "custom", "honeyed",
        "refined", "rustic", "authentic", "steamed", "fragrant", "whipped", "energetic",
        "casual", "light", "zingy", "smoky", "lively", "vibrant", "exuberant", "exhilarating",
        "inspiring", "brisk", "edgy", "trendy", "artful", "imaginative", "captivating",
        "dashing", "peppy", "perky", "lustrous", "magical", "enchanting", "euphoric",
        "jubilant", "blissful", "serene", "tranquil", "graceful", "eloquent", "sophisticated",
        "chic", "elegant", "fashionable", "stylish", "swanky", "ritzy", "plush", "lavish",
        "posh", "deluxe", "opulent", "grand", "magnificent", "sumptuous", "luxurious", "classy",
        "swish", "splendid", "majestic", "regal", "stately", "august", "imposing", "impressive",
        "monumental", "grandiose", "heroic", "epic", "legendary", "iconic", "celebrated",
        "renowned", "famous", "notable", "eminent", "distinguished", "prestigious", "acclaimed",
        "esteemed", "respected", "honored", "venerated", "revered", "admired", "cherished",
        "beloved", "valued", "prized", "treasured", "appreciated", "recognized", "acknowledged",
        "commended", "praised", "lauded", "applauded", "celebrated", "extolled", "glorified",
        "exalted", "deified", "idolized", "worshipped", "reverenced", "venerated", "hallowed",
        "sanctified", "blessed", "consecrated", "anointed", "canonized", "beatified", "exemplary",
        "paragon", "model", "archetype", "epitome", "quintessence", "personification", "embodiment",
        "incarnation", "manifestation", "avatar", "prodigy", "virtuoso", "maestro", "master",
        "guru", "sage", "wizard", "magician", "enchanter", "sorcerer", "necromancer", "alchemist",
        "conjurer", "warlock", "witch", "seer", "prophet", "oracle", "mystic", "clairvoyant",
        "psychic", "medium", "shaman", "healer", "therapist", "counselor", "guide", "mentor",
        "teacher", "instructor", "trainer", "coach", "tutor", "educator", "lecturer", "preacher",
        "evangelist", "missionary", "crusader"
    ]

    NOUNS = [
        "Espresso", "Latte", "Cappuccino", "Mocha", "Americano", "Macchiato", "Ristretto",
        "Frappuccino", "Breve", "Cortado", "Doppio", "Affogato", "Lungo", "FlatWhite",
        "RedEye", "BlackCoffee", "DripCoffee", "FilterCoffee", "ColdBrew", "NitroCoffee",
        "IcedCoffee", "TurkishCoffee", "GreekCoffee", "IrishCoffee", "VienneseCoffee",
        "CafeAuLait", "CafeConLeche", "CafeCrema", "CafeCubano", "CafeZorro",
        "Galao", "CoffeeMilk", "EggnogLatte", "PumpkinSpiceLatte", "ChaiLatte",
        "CaramelMacchiato", "VanillaLatte", "MochaLatte", "HazelnutLatte", "ToffeeNutLatte",
        "MapleLatte", "GingerbreadLatte", "HoneyLatte", "SoyLatte", "AlmondMilkLatte",
        "OatMilkLatte", "RiceMilkLatte", "CoconutMilkLatte", "EspressoConPanna", "EspressoMacchiato",
        "EspressoTonic", "EspressoRomano", "EspressoMartini", "EspressoShot", "DecafEspresso",
        "Barista", "CoffeeGrinder", "CoffeeMaker", "EspressoMachine", "FrenchPress",
        "Aeropress", "MokaPot", "Percolator", "SiphonCoffeeMaker", "CoffeeRoaster",
        "CoffeeFilter", "CoffeeScoop", "Tamper", "MilkFrother", "SteamWand",
        "KnockBox", "Portafilter", "GroupHead", "CoffeeScale", "ShotGlass",
        "CappuccinoCup", "LatteMug", "EspressoCup", "Demitasse", "CoffeeMug",
        "TravelMug", "CoffeeThermos", "CoffeePot", "CoffeeCanister", "CoffeeBeans",
        "GroundCoffee", "CoffeeCapsule", "CoffeePod", "CoffeePuck", "CoffeeGrounds",
        "RoastedBeans", "GreenBeans", "SingleOrigin", "CoffeeBlend", "FairTradeCoffee",
        "OrganicCoffee", "ShadeGrownCoffee", "KonaCoffee", "ArabicaBeans", "RobustaBeans",
        "LightRoast", "MediumRoast", "DarkRoast", "FrenchRoast", "ItalianRoast",
        "EspressoRoast", "CoffeeCreamer", "HalfAndHalf", "WhippedCream", "Milk",
        "SoyMilk", "AlmondMilk", "OatMilk", "RiceMilk", "CoconutMilk",
        "Sugar", "BrownSugar", "Honey", "MapleSyrup", "AgaveNectar",
        "VanillaSyrup", "CaramelSyrup", "HazelnutSyrup", "AlmondSyrup", "ChocolateSyrup",
        "Cinnamon", "Nutmeg", "CocoaPowder", "ChocolatePowder", "WhippedTopping",
        "CoffeeStirrer", "CoffeeSpoon", "CoffeeStraw", "Napkin", "Coaster",
        "CoffeeTable", "BarStool", "CafeChair", "Sofa", "CoffeeShop",
        "Cafe", "EspressoBar", "CoffeeStand", "CoffeeKiosk", "CoffeeCart",
        "CoffeeTruck", "CoffeeFestival", "CoffeeTasting", "CoffeeWorkshop", "CoffeeCourse",
        "LatteArt", "CoffeeArt", "Cupping", "Brewing", "Grinding",
        "Tamping", "Frothing", "Steaming", "Pouring", "Serving",
        "Sip", "Guzzle", "Taste", "Savor", "Enjoy",
        "Crave", "Indulge", "Relax", "Unwind", "Socialize",
        "Chat", "Meet", "Gather", "Connect", "Network",
        "Caffeine", "Decaffeination", "Aroma", "Flavor", "Body",
        "Acidity", "Aftertaste", "Crema", "Foam", "Bloom",
        "Extraction", "OverExtraction", "UnderExtraction", "BrewRatio", "Dose",
        "Yield", "Temperature", "Pressure", "Consistency", "Quality",
        "Freshness", "Origin", "Terroir", "Altitude", "Harvest",
        "Processing", "Washing", "Drying", "Fermentation", "Pulping",
        "Hulling", "Sorting", "Grading", "CuppingScore", "FlavorProfile",
        "TastingNotes", "AromaWheel", "CoffeeCulture", "CoffeeHistory", "CoffeeTradition",
        "CoffeeCeremony", "CoffeeRitual", "CoffeeBreak", "CoffeeHour", "CoffeeDate",
        "CoffeeLover", "CoffeeEnthusiast", "CoffeeAficionado", "CoffeeConnoisseur", "CoffeeGeek",
        "CoffeeSnob", "CoffeeAddict", "CoffeeHabit", "CoffeeRoutine", "CoffeeExperience"
    ]
