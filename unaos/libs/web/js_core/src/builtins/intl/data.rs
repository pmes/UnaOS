//! SUPERSEDED by INTLCLDR (SR137): nothing in the engine reads this file any more — the locale data is the
//! generated CLDR 46 table of `crate::cldr`, the zones the generated tzdata 2025b table of `crate::tzdata`. Kept
//! (R20) until the seat retires it; `builtins/temporal/gen_tzdata.py` still parses its ZONES list.
//!
//! The locale data of `en`, `en-US` and `en-GB` (values from CLDR 46 as Chromium 13x reports them), and
//! the time zones as POSIX rules (the CURRENT rule of each IANA zone, tzdata 2025b; historical transitions
//! are not modelled — a date before a zone's last rule change formats with today's rule).

pub struct Currency {
    pub code: &'static str,
    /// ISO 4217 minor units (CLDR `currencyData/fractions` where it differs for display).
    pub digits: u32,
    pub symbol: &'static str,
    pub narrow: &'static str,
    pub one: &'static str,
    pub other: &'static str,
}

macro_rules! cur {
    ($($c:literal, $d:literal, $s:literal, $n:literal, $o:literal, $p:literal;)*) => {
        pub static CURRENCIES: &[Currency] = &[$(Currency { code: $c, digits: $d, symbol: $s, narrow: $n, one: $o, other: $p },)*];
    };
}

cur!(
    "AED", 2, "AED", "AED", "UAE dirham", "UAE dirhams";
    "ARS", 2, "ARS", "$", "Argentine peso", "Argentine pesos";
    "AUD", 2, "A$", "$", "Australian dollar", "Australian dollars";
    "BHD", 3, "BHD", "BHD", "Bahraini dinar", "Bahraini dinars";
    "BRL", 2, "R$", "R$", "Brazilian real", "Brazilian reals";
    "CAD", 2, "CA$", "$", "Canadian dollar", "Canadian dollars";
    "CHF", 2, "CHF", "CHF", "Swiss franc", "Swiss francs";
    "CLP", 0, "CLP", "$", "Chilean peso", "Chilean pesos";
    "CNY", 2, "CN¥", "¥", "Chinese yuan", "Chinese yuan";
    "COP", 2, "COP", "$", "Colombian peso", "Colombian pesos";
    "CZK", 2, "CZK", "Kč", "Czech koruna", "Czech korunas";
    "DKK", 2, "DKK", "kr", "Danish krone", "Danish kroner";
    "EGP", 2, "EGP", "E£", "Egyptian pound", "Egyptian pounds";
    "EUR", 2, "€", "€", "euro", "euros";
    "GBP", 2, "£", "£", "British pound", "British pounds";
    "HKD", 2, "HK$", "$", "Hong Kong dollar", "Hong Kong dollars";
    "HUF", 2, "HUF", "Ft", "Hungarian forint", "Hungarian forints";
    "IDR", 2, "IDR", "Rp", "Indonesian rupiah", "Indonesian rupiahs";
    "ILS", 2, "₪", "₪", "Israeli new shekel", "Israeli new shekels";
    "INR", 2, "₹", "₹", "Indian rupee", "Indian rupees";
    "ISK", 0, "ISK", "kr", "Icelandic króna", "Icelandic krónur";
    "JOD", 3, "JOD", "JOD", "Jordanian dinar", "Jordanian dinars";
    "JPY", 0, "¥", "¥", "Japanese yen", "Japanese yen";
    "KRW", 0, "₩", "₩", "South Korean won", "South Korean won";
    "KWD", 3, "KWD", "KWD", "Kuwaiti dinar", "Kuwaiti dinars";
    "MXN", 2, "MX$", "$", "Mexican peso", "Mexican pesos";
    "MYR", 2, "MYR", "RM", "Malaysian ringgit", "Malaysian ringgits";
    "NGN", 2, "NGN", "₦", "Nigerian naira", "Nigerian nairas";
    "NOK", 2, "NOK", "kr", "Norwegian krone", "Norwegian kroner";
    "NZD", 2, "NZ$", "$", "New Zealand dollar", "New Zealand dollars";
    "OMR", 3, "OMR", "OMR", "Omani rial", "Omani rials";
    "PHP", 2, "₱", "₱", "Philippine peso", "Philippine pesos";
    "PKR", 2, "PKR", "Rs", "Pakistani rupee", "Pakistani rupees";
    "PLN", 2, "PLN", "zł", "Polish zloty", "Polish zlotys";
    "RUB", 2, "RUB", "₽", "Russian ruble", "Russian rubles";
    "SAR", 2, "SAR", "SAR", "Saudi riyal", "Saudi riyals";
    "SEK", 2, "SEK", "kr", "Swedish krona", "Swedish kronor";
    "SGD", 2, "SGD", "$", "Singapore dollar", "Singapore dollars";
    "THB", 2, "THB", "฿", "Thai baht", "Thai baht";
    "TND", 3, "TND", "TND", "Tunisian dinar", "Tunisian dinars";
    "TRY", 2, "TRY", "₺", "Turkish lira", "Turkish Lira";
    "TWD", 2, "NT$", "$", "New Taiwan dollar", "New Taiwan dollars";
    "UAH", 2, "UAH", "₴", "Ukrainian hryvnia", "Ukrainian hryvnias";
    "USD", 2, "$", "$", "US dollar", "US dollars";
    "VND", 0, "₫", "₫", "Vietnamese dong", "Vietnamese dong";
    "XAF", 0, "FCFA", "FCFA", "Central African CFA franc", "Central African CFA francs";
    "XOF", 0, "F\u{202F}CFA", "F\u{202F}CFA", "West African CFA franc", "West African CFA francs";
    "ZAR", 2, "ZAR", "R", "South African rand", "South African rand";
);

pub fn currency(code: &str) -> Option<&'static Currency> {
    CURRENCIES.iter().find(|c| c.code == code)
}

/// en-GB's symbol where it differs from en-US.
pub fn gb_symbol(code: &str) -> Option<&'static str> {
    match code {
        "USD" => Some("US$"),
        _ => None,
    }
}

/// A sanctioned simple unit (ECMA-402 Table 2) with its en display patterns (`{0}` is the number).
pub struct Unit {
    pub id: &'static str,
    pub short_one: &'static str,
    pub short_other: &'static str,
    pub narrow: &'static str,
    pub long_one: &'static str,
    pub long_other: &'static str,
}

macro_rules! units {
    ($($id:literal, $so:literal, $sp:literal, $n:literal, $lo:literal, $lp:literal;)*) => {
        pub static UNITS: &[Unit] = &[$(Unit { id: $id, short_one: $so, short_other: $sp, narrow: $n, long_one: $lo, long_other: $lp },)*];
    };
}

units!(
    "acre", "{0} ac", "{0} ac", "{0}ac", "{0} acre", "{0} acres";
    "bit", "{0} bit", "{0} bit", "{0}bit", "{0} bit", "{0} bits";
    "byte", "{0} byte", "{0} byte", "{0}B", "{0} byte", "{0} bytes";
    "celsius", "{0}°C", "{0}°C", "{0}°C", "{0} degree Celsius", "{0} degrees Celsius";
    "centimeter", "{0} cm", "{0} cm", "{0}cm", "{0} centimeter", "{0} centimeters";
    "day", "{0} day", "{0} days", "{0}d", "{0} day", "{0} days";
    "degree", "{0} deg", "{0} deg", "{0}°", "{0} degree", "{0} degrees";
    "fahrenheit", "{0}°F", "{0}°F", "{0}°", "{0} degree Fahrenheit", "{0} degrees Fahrenheit";
    "fluid-ounce", "{0} fl oz", "{0} fl oz", "{0}fl oz", "{0} fluid ounce", "{0} fluid ounces";
    "foot", "{0} ft", "{0} ft", "{0}′", "{0} foot", "{0} feet";
    "gallon", "{0} gal", "{0} gal", "{0}gal", "{0} gallon", "{0} gallons";
    "gigabit", "{0} Gb", "{0} Gb", "{0}Gb", "{0} gigabit", "{0} gigabits";
    "gigabyte", "{0} GB", "{0} GB", "{0}GB", "{0} gigabyte", "{0} gigabytes";
    "gram", "{0} g", "{0} g", "{0}g", "{0} gram", "{0} grams";
    "hectare", "{0} ha", "{0} ha", "{0}ha", "{0} hectare", "{0} hectares";
    "hour", "{0} hr", "{0} hr", "{0}h", "{0} hour", "{0} hours";
    "inch", "{0} in", "{0} in", "{0}″", "{0} inch", "{0} inches";
    "kilobit", "{0} kb", "{0} kb", "{0}kb", "{0} kilobit", "{0} kilobits";
    "kilobyte", "{0} kB", "{0} kB", "{0}kB", "{0} kilobyte", "{0} kilobytes";
    "kilogram", "{0} kg", "{0} kg", "{0}kg", "{0} kilogram", "{0} kilograms";
    "kilometer", "{0} km", "{0} km", "{0}km", "{0} kilometer", "{0} kilometers";
    "liter", "{0} L", "{0} L", "{0}L", "{0} liter", "{0} liters";
    "megabit", "{0} Mb", "{0} Mb", "{0}Mb", "{0} megabit", "{0} megabits";
    "megabyte", "{0} MB", "{0} MB", "{0}MB", "{0} megabyte", "{0} megabytes";
    "meter", "{0} m", "{0} m", "{0}m", "{0} meter", "{0} meters";
    "microsecond", "{0} μs", "{0} μs", "{0}μs", "{0} microsecond", "{0} microseconds";
    "mile", "{0} mi", "{0} mi", "{0}mi", "{0} mile", "{0} miles";
    "mile-scandinavian", "{0} smi", "{0} smi", "{0}smi", "{0} mile-scandinavian", "{0} miles-scandinavian";
    "milliliter", "{0} mL", "{0} mL", "{0}mL", "{0} milliliter", "{0} milliliters";
    "millimeter", "{0} mm", "{0} mm", "{0}mm", "{0} millimeter", "{0} millimeters";
    "millisecond", "{0} ms", "{0} ms", "{0}ms", "{0} millisecond", "{0} milliseconds";
    "minute", "{0} min", "{0} min", "{0}m", "{0} minute", "{0} minutes";
    "month", "{0} mth", "{0} mths", "{0}m", "{0} month", "{0} months";
    "nanosecond", "{0} ns", "{0} ns", "{0}ns", "{0} nanosecond", "{0} nanoseconds";
    "ounce", "{0} oz", "{0} oz", "{0}oz", "{0} ounce", "{0} ounces";
    "percent", "{0}%", "{0}%", "{0}%", "{0} percent", "{0} percent";
    "petabyte", "{0} PB", "{0} PB", "{0}PB", "{0} petabyte", "{0} petabytes";
    "pound", "{0} lb", "{0} lb", "{0}#", "{0} pound", "{0} pounds";
    "second", "{0} sec", "{0} sec", "{0}s", "{0} second", "{0} seconds";
    "stone", "{0} st", "{0} st", "{0}st", "{0} stone", "{0} stones";
    "terabit", "{0} Tb", "{0} Tb", "{0}Tb", "{0} terabit", "{0} terabits";
    "terabyte", "{0} TB", "{0} TB", "{0}TB", "{0} terabyte", "{0} terabytes";
    "week", "{0} wk", "{0} wks", "{0}w", "{0} week", "{0} weeks";
    "yard", "{0} yd", "{0} yd", "{0}yd", "{0} yard", "{0} yards";
    "year", "{0} yr", "{0} yrs", "{0}y", "{0} year", "{0} years";
);

pub fn unit(id: &str) -> Option<&'static Unit> {
    UNITS.iter().find(|u| u.id == id)
}

/// CLDR's own compound patterns (short, narrow, long one, long other) where the generic `X/Y` rule differs.
pub fn compound_unit(id: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    Some(match id {
        "kilometer-per-hour" => ("{0} km/h", "{0}km/h", "{0} kilometer per hour", "{0} kilometers per hour"),
        "mile-per-hour" => ("{0} mph", "{0}mph", "{0} mile per hour", "{0} miles per hour"),
        "meter-per-second" => ("{0} m/s", "{0}m/s", "{0} meter per second", "{0} meters per second"),
        "mile-per-gallon" => ("{0} mpg", "{0}mpg", "{0} mile per gallon", "{0} miles per gallon"),
        "liter-per-kilometer" => ("{0} L/km", "{0}L/km", "{0} liter per kilometer", "{0} liters per kilometer"),
        _ => return None,
    })
}

pub static MONTHS_LONG: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
pub static MONTHS_SHORT: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
pub static MONTHS_NARROW: [&str; 12] = ["J", "F", "M", "A", "M", "J", "J", "A", "S", "O", "N", "D"];
pub static DAYS_LONG: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
pub static DAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
pub static DAYS_NARROW: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];

/// A metazone's names (CLDR `metaZones` en): abbreviations used in en-US, in en-GB, and the long forms.
pub struct Meta {
    pub abbr_std: &'static str,
    pub abbr_dst: &'static str,
    /// The abbreviations are what `timeZoneName: "short"` shows in en-US (North American metazones).
    pub us_abbr: bool,
    /// ... and in en-GB (the European metazones, which CLDR en_GB names by abbreviation).
    pub gb_abbr: bool,
    pub long_std: &'static str,
    pub long_dst: &'static str,
    pub generic_long: &'static str,
    pub generic_short: &'static str,
}

macro_rules! metas {
    ($($as:literal, $ad:literal, $us:literal, $gb:literal, $ls:literal, $ld:literal, $gl:literal, $gs:literal;)*) => {
        pub static METAS: &[Meta] = &[$(Meta { abbr_std: $as, abbr_dst: $ad, us_abbr: $us, gb_abbr: $gb, long_std: $ls, long_dst: $ld, generic_long: $gl, generic_short: $gs },)*];
    };
}

metas!(
    "", "", false, false, "", "", "", "";
    "EST", "EDT", true, true, "Eastern Standard Time", "Eastern Daylight Time", "Eastern Time", "ET";
    "CST", "CDT", true, true, "Central Standard Time", "Central Daylight Time", "Central Time", "CT";
    "MST", "MDT", true, true, "Mountain Standard Time", "Mountain Daylight Time", "Mountain Time", "MT";
    "PST", "PDT", true, true, "Pacific Standard Time", "Pacific Daylight Time", "Pacific Time", "PT";
    "AKST", "AKDT", true, true, "Alaska Standard Time", "Alaska Daylight Time", "Alaska Time", "AKT";
    "HST", "HDT", true, true, "Hawaii-Aleutian Standard Time", "Hawaii-Aleutian Daylight Time", "Hawaii-Aleutian Time", "HST";
    "AST", "ADT", false, false, "Atlantic Standard Time", "Atlantic Daylight Time", "Atlantic Time", "AT";
    "NST", "NDT", false, false, "Newfoundland Standard Time", "Newfoundland Daylight Time", "Newfoundland Time", "NT";
    "GMT", "BST", false, true, "Greenwich Mean Time", "British Summer Time", "United Kingdom Time", "GMT";
    "WET", "WEST", false, true, "Western European Standard Time", "Western European Summer Time", "Western European Time", "WET";
    "CET", "CEST", false, true, "Central European Standard Time", "Central European Summer Time", "Central European Time", "CET";
    "EET", "EEST", false, true, "Eastern European Standard Time", "Eastern European Summer Time", "Eastern European Time", "EET";
    "MSK", "MSK", false, false, "Moscow Standard Time", "Moscow Summer Time", "Moscow Time", "MSK";
    "IST", "IST", false, false, "India Standard Time", "India Standard Time", "India Standard Time", "IST";
    "CST", "CDT", false, false, "China Standard Time", "China Daylight Time", "China Time", "CST";
    "JST", "JDT", false, false, "Japan Standard Time", "Japan Daylight Time", "Japan Time", "JST";
    "KST", "KDT", false, false, "Korean Standard Time", "Korean Daylight Time", "Korean Time", "KST";
    "AEST", "AEDT", false, false, "Australian Eastern Standard Time", "Australian Eastern Daylight Time", "Eastern Australia Time", "AET";
    "ACST", "ACDT", false, false, "Australian Central Standard Time", "Australian Central Daylight Time", "Central Australia Time", "ACT";
    "AWST", "AWDT", false, false, "Australian Western Standard Time", "Australian Western Daylight Time", "Western Australia Time", "AWT";
    "NZST", "NZDT", false, false, "New Zealand Standard Time", "New Zealand Daylight Time", "New Zealand Time", "NZT";
    "IST", "IDT", false, false, "Israel Standard Time", "Israel Daylight Time", "Israel Time", "IT";
    "GMT", "IST", false, true, "Greenwich Mean Time", "Irish Standard Time", "Ireland Time", "GMT";
    "SAST", "SAST", false, false, "South Africa Standard Time", "South Africa Standard Time", "South Africa Standard Time", "SAST";
    "HKT", "HKST", false, false, "Hong Kong Standard Time", "Hong Kong Summer Time", "Hong Kong Time", "HKT";
);

/// An IANA zone: its id, the POSIX rule of its current offsets, and its metazone (index into METAS, 0 none).
pub struct Zone {
    pub id: &'static str,
    pub posix: &'static str,
    pub meta: u8,
}

macro_rules! zones {
    ($($id:literal, $p:literal, $m:literal;)*) => {
        pub static ZONES: &[Zone] = &[$(Zone { id: $id, posix: $p, meta: $m },)*];
    };
}

zones!(
    "Africa/Cairo", "EET-2EEST,M4.5.5/0,M10.5.4/24", 12;
    "Africa/Casablanca", "<+01>-1", 0;
    "Africa/Johannesburg", "SAST-2", 24;
    "Africa/Lagos", "WAT-1", 0;
    "Africa/Nairobi", "EAT-3", 0;
    "America/Anchorage", "AKST9AKDT,M3.2.0,M11.1.0", 5;
    "America/Argentina/Buenos_Aires", "<-03>3", 0;
    "America/Bogota", "<-05>5", 0;
    "America/Boise", "MST7MDT,M3.2.0,M11.1.0", 3;
    "America/Caracas", "<-04>4", 0;
    "America/Chicago", "CST6CDT,M3.2.0,M11.1.0", 2;
    "America/Denver", "MST7MDT,M3.2.0,M11.1.0", 3;
    "America/Detroit", "EST5EDT,M3.2.0,M11.1.0", 1;
    "America/Edmonton", "MST7MDT,M3.2.0,M11.1.0", 3;
    "America/Halifax", "AST4ADT,M3.2.0,M11.1.0", 7;
    "America/Lima", "<-05>5", 0;
    "America/Los_Angeles", "PST8PDT,M3.2.0,M11.1.0", 4;
    "America/Mexico_City", "CST6", 2;
    "America/New_York", "EST5EDT,M3.2.0,M11.1.0", 1;
    "America/Phoenix", "MST7", 3;
    "America/Puerto_Rico", "AST4", 7;
    "America/Santiago", "<-04>4<-03>,M9.1.6/24,M4.1.6/24", 0;
    "America/Sao_Paulo", "<-03>3", 0;
    "America/St_Johns", "NST3:30NDT,M3.2.0,M11.1.0", 8;
    "America/Toronto", "EST5EDT,M3.2.0,M11.1.0", 1;
    "America/Vancouver", "PST8PDT,M3.2.0,M11.1.0", 4;
    "America/Winnipeg", "CST6CDT,M3.2.0,M11.1.0", 2;
    "Asia/Bangkok", "<+07>-7", 0;
    "Asia/Dhaka", "<+06>-6", 0;
    "Asia/Dubai", "<+04>-4", 0;
    "Asia/Ho_Chi_Minh", "<+07>-7", 0;
    "Asia/Hong_Kong", "HKT-8", 25;
    "Asia/Jakarta", "WIB-7", 0;
    "Asia/Jerusalem", "IST-2IDT,M3.4.4/26,M10.5.0", 22;
    "Asia/Karachi", "PKT-5", 0;
    "Asia/Kathmandu", "<+0545>-5:45", 0;
    "Asia/Kolkata", "IST-5:30", 14;
    "Asia/Manila", "PST-8", 0;
    "Asia/Riyadh", "<+03>-3", 0;
    "Asia/Seoul", "KST-9", 17;
    "Asia/Shanghai", "CST-8", 15;
    "Asia/Singapore", "<+08>-8", 0;
    "Asia/Taipei", "CST-8", 0;
    "Asia/Tehran", "<+0330>-3:30", 0;
    "Asia/Tokyo", "JST-9", 16;
    "Atlantic/Reykjavik", "GMT0", 0;
    "Australia/Adelaide", "ACST-9:30ACDT,M10.1.0,M4.1.0/3", 19;
    "Australia/Brisbane", "AEST-10", 18;
    "Australia/Darwin", "ACST-9:30", 19;
    "Australia/Hobart", "AEST-10AEDT,M10.1.0,M4.1.0/3", 18;
    "Australia/Melbourne", "AEST-10AEDT,M10.1.0,M4.1.0/3", 18;
    "Australia/Perth", "AWST-8", 20;
    "Australia/Sydney", "AEST-10AEDT,M10.1.0,M4.1.0/3", 18;
    "Europe/Amsterdam", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Athens", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Belgrade", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Berlin", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Brussels", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Bucharest", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Budapest", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Copenhagen", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Dublin", "IST-1GMT0,M10.5.0,M3.5.0/1", 23;
    "Europe/Helsinki", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Istanbul", "<+03>-3", 0;
    "Europe/Kyiv", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Lisbon", "WET0WEST,M3.5.0/1,M10.5.0", 10;
    "Europe/London", "GMT0BST,M3.5.0/1,M10.5.0", 9;
    "Europe/Madrid", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Moscow", "MSK-3", 13;
    "Europe/Oslo", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Paris", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Prague", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Riga", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Rome", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Sofia", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Stockholm", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Tallinn", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Vienna", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Vilnius", "EET-2EEST,M3.5.0/3,M10.5.0/4", 12;
    "Europe/Warsaw", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Europe/Zurich", "CET-1CEST,M3.5.0,M10.5.0/3", 11;
    "Pacific/Auckland", "NZST-12NZDT,M9.5.0,M4.1.0/3", 21;
    "Pacific/Fiji", "<+12>-12", 0;
    "Pacific/Honolulu", "HST10", 6;
);

/// Case-insensitive zone lookup (§6.5.1 IsValidTimeZoneName / GetAvailableNamedTimeZoneIdentifier).
pub fn zone(id: &str) -> Option<&'static Zone> {
    ZONES.iter().find(|z| z.id.eq_ignore_ascii_case(id))
}
