// CS2's killfeed icons and the names shown for them. Keys are the names killfeed
// analysis uses: weapons by CS2 class name, modifiers as the API returns them.

const ICONS = import.meta.glob<string>("./icons/*.svg", {
  eager: true,
  query: "?no-inline",
  import: "default",
});

/** URL of the icon for a weapon or modifier, if CS2 has one. */
export function iconUrl(name: string): string | undefined {
  return ICONS[`./icons/${name}.svg`];
}

const WEAPONS: Record<string, string> = {
  ak47: "AK-47",
  aug: "AUG",
  awp: "AWP",
  bayonet: "Bayonet",
  bizon: "PP-Bizon",
  cz75a: "CZ75-Auto",
  deagle: "Desert Eagle",
  decoy: "Decoy Grenade",
  elite: "Dual Berettas",
  famas: "FAMAS",
  firebomb: "Fire Bomb",
  fiveseven: "Five-SeveN",
  flashbang: "Flashbang",
  g3sg1: "G3SG1",
  galilar: "Galil AR",
  glock: "Glock-18",
  hegrenade: "HE Grenade",
  hkp2000: "P2000",
  incgrenade: "Incendiary Grenade",
  inferno: "Fire",
  knife_butterfly: "Butterfly Knife",
  knife_canis: "Survival Knife",
  knife_cord: "Paracord Knife",
  knife_css: "Classic Knife",
  knife_falchion: "Falchion Knife",
  knife_flip: "Flip Knife",
  knife_gut: "Gut Knife",
  knife_gypsy_jackknife: "Navaja Knife",
  knife_karambit: "Karambit",
  knife_kukri: "Kukri Knife",
  knife_m9_bayonet: "M9 Bayonet",
  knife_outdoor: "Nomad Knife",
  knife_push: "Shadow Daggers",
  knife_skeleton: "Skeleton Knife",
  knife_stiletto: "Stiletto Knife",
  knife_survival_bowie: "Bowie Knife",
  knife_tactical: "Huntsman Knife",
  knife_ursus: "Ursus Knife",
  knife_widowmaker: "Talon Knife",
  knifegg: "Golden Knife",
  m249: "M249",
  m4a1: "M4A4",
  m4a1_silencer: "M4A1-S",
  mac10: "MAC-10",
  mag7: "MAG-7",
  molotov: "Molotov",
  mp5sd: "MP5-SD",
  mp7: "MP7",
  mp9: "MP9",
  negev: "Negev",
  nova: "Nova",
  p250: "P250",
  p90: "P90",
  prop_exploding_barrel: "Exploding barrel",
  revolver: "R8 Revolver",
  sawedoff: "Sawed-Off",
  scar20: "SCAR-20",
  sg556: "SG 553",
  smokegrenade: "Smoke Grenade",
  ssg08: "SSG 08",
  stomp_damage: "Stomp",
  suicide: "Suicide",
  taser: "Zeus x27",
  tec9: "Tec-9",
  ump45: "UMP-45",
  usp_silencer: "USP-S",
  xm1014: "XM1014",
};

const MODIFIERS: Record<string, string> = {
  headshot: "headshot",
  wallbang: "wallbang",
  through_smoke: "through smoke",
  noscope: "no-scope",
  blind: "while blind",
  in_air: "in the air",
  flash_assist: "flash assist",
  domination: "domination",
  revenge: "revenge",
};

/** "AK-47"; any other knife reads as "Knife". */
export function weaponName(weapon: string | null | undefined): string {
  if (!weapon) return "Unknown weapon";
  return WEAPONS[weapon] ?? (weapon.startsWith("knife") ? "Knife" : weapon);
}

export function modifierName(modifier: string): string {
  return MODIFIERS[modifier] ?? modifier.replaceAll("_", " ");
}
