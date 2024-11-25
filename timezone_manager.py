# import json
import subprocess


class TimezoneManager:

    @staticmethod
    def get_organized_timezones():
        '''
            make use of linux embedded timezone information to compile and provide
            the user information to make the timezone selection easier

            return: json containing timezones grouped by country and identified by zone

            i.e

            {
            ...
                "Mexico":{
                    "Bahia Banderas": "America/Bahia_Banderas",
                    "Cancun": "America/Cancun",
                    "Chihuahua": "America/Chihuahua",
                    "Ciudad Juarez": "America/Ciudad_Juarez",
                    ...
                }
            }
        '''
        # Read the country -> timezones mapping from /usr/share/zoneinfo/zone.tab
        country_to_timezones = {}

        # Create a map from country code to country name
        country_code_to_name = {}

        # map country code to country name
        with open('/usr/share/zoneinfo/iso3166.tab') as f:
            for line in f:
                # Skip comments and empty lines
                if line.startswith('#') or not line.strip():
                    continue
                parts = line.split('\t')
                country_code = parts[0]
                country_name = parts[1].rstrip('\n')
                country_code_to_name.setdefault(country_code, country_name)

        #map timezones to country code
        with open('/usr/share/zoneinfo/zone.tab') as f:
            for line in f:
                # Skip comments and empty lines
                if line.startswith('#') or not line.strip():
                    continue
                parts = line.split('\t')
                country_code = parts[0]
                timezone = parts[2].rstrip()
                country_name = country_code_to_name.get(country_code, "Unknown Country")
                country_to_timezones.setdefault(country_name, []).append(timezone)

        # Group timezones by country
        country_timezones = {}
        try:
            result = subprocess.run(
                ['timedatectl', 'list-timezones'],
                stdout=subprocess.PIPE,   # Capture the standard output
                stderr=subprocess.PIPE,   # Capture the standard error
                text=True,                # Decode output as a string
                check=True                # Raise an exception on non-zero exit codes
            )
            _all_timezones = result.stdout.splitlines()

            for tz in _all_timezones:
                fallback_subregion = tz.split('/')[0] # if there is no sub-region, take the region as sub region
                zone = tz.split('/')[1:]  # Skip the first segment (Region)

                # separate words liked by "_"
                zone = [
                    " ".join(zone[i].split("_")) for i in range(len(zone))
                ]

                if(zone.__len__() > 0):
                    sub_region = ' - '.join(zone)  # Combine state/city into "state - city" format
                else:
                    sub_region = ''.join(fallback_subregion)  # Combine state/city into "state - city" format

                if tz.startswith('Etc/'):
                    country_timezones.setdefault('ETC', {})[sub_region] = tz
                elif tz in sum(country_to_timezones.values(), []):  # Flatten the country -> timezone mapping
                    for country, timezones in country_to_timezones.items():
                        if tz in timezones:
                            # if the first sub-region is the same as the country (Argentina, looking at You) don't 
                            # keep that as subregion
                            if zone[0] == country and len(zone) > 1:
                                sub_region = ' - '.join(zone[1:])
                            country_timezones.setdefault(country, {})[sub_region] = tz
                            break
                else:
                    country_timezones.setdefault('LINKS', {})[sub_region] = tz

            # Print the result in JSON format
            # print(json.dumps(country_timezones, indent=2))
            return(country_timezones)

        except:
            _all_timezones = []