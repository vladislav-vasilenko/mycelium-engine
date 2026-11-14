[out:json][timeout:180];
(
  way["waterway"~"^(river|stream|waterfall)$"](30.20,130.36,30.48,130.69);
  way["highway"](30.20,130.36,30.48,130.69);
  node["waterway"="waterfall"](30.20,130.36,30.48,130.69);
  node["natural"~"^(peak|tree|waterfall)$"](30.20,130.36,30.48,130.69);
  node["tourism"~"^(alpine_hut|wilderness_hut|camp_site|viewpoint)$"](30.20,130.36,30.48,130.69);
  node["amenity"="shelter"](30.20,130.36,30.48,130.69);
  way["tourism"~"^(alpine_hut|wilderness_hut|camp_site)$"](30.20,130.36,30.48,130.69);
  way["natural"~"^(wetland|water)$"](30.20,130.36,30.48,130.69);
);
out geom;
