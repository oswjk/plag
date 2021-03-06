/*
    Copyright (C) 2018 Oskari Timperi <oskari.timperi@iki.fi>

    This program is free software: you can redistribute it and/or modify
    it under the terms of the GNU General Public License as published by
    the Free Software Foundation, either version 3 of the License, or
    (at your option) any later version.

    This program is distributed in the hope that it will be useful,
    but WITHOUT ANY WARRANTY; without even the implied warranty of
    MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
    GNU General Public License for more details.

    You should have received a copy of the GNU General Public License
    along with this program.  If not, see <https://www.gnu.org/licenses/>.
*/

extern crate geojson;
extern crate geo_types;
extern crate exif;
extern crate serde_json;
#[macro_use]
extern crate clap;

use std::path::Path;

use geojson::{Feature, GeoJson, Geometry, Value, FeatureCollection};
use serde_json::{Map, to_value};

#[derive(Debug)]
enum Error {
    IoError(std::io::Error),
    Utf8Error(std::str::Utf8Error),
    FieldMissing(exif::Tag),
    InvalidField(exif::Tag, &'static str),
    ExifError(exif::Error),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Error::IoError(error) => write!(f, "{}", error),
            Error::Utf8Error(error) => write!(f, "{}", error),
            Error::FieldMissing(tag) => write!(f, "missing field: {}", tag),
            Error::InvalidField(tag, msg) => write!(f, "invalid field {}: {}", tag, msg),
            Error::ExifError(error) => write!(f, "{}", error),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::IoError(e) => Some(e),
            Error::Utf8Error(e) => Some(e),
            Error::ExifError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::str::Utf8Error> for Error {
    fn from(value: std::str::Utf8Error) -> Error {
        Error::Utf8Error(value)
    }
}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Error {
        Error::IoError(value)
    }
}

impl From<exif::Error> for Error {
    fn from(value: exif::Error) -> Error {
        Error::ExifError(value)
    }
}

type Result<T> = std::result::Result<T, Error>;

fn get_degrees(reader: &exif::Reader, tag: exif::Tag) -> Result<f64> {
    let field = reader.get_field(tag, false).ok_or(Error::FieldMissing(tag))?;

    match field.value {
        exif::Value::Rational(ref dms) => {
            if dms.len() != 3 {
                return Err(Error::InvalidField(tag, "expected 3 rationals"))
            }
            let degrees = dms[0].to_f64();
            let min = dms[1].to_f64();
            let sec = dms[2].to_f64();
            Ok(degrees + min/60.0 + sec/3600.0)
        },
        _ => Err(Error::InvalidField(tag, "invalid field type"))
    }
}

fn get_string(reader: &exif::Reader, tag: exif::Tag) -> Result<&str> {
    let field = reader.get_field(tag, false).ok_or(Error::FieldMissing(tag))?;
    if let exif::Value::Ascii(ref s) = field.value {
        let s = s[0];
        std::str::from_utf8(s).map_err(|err| err.into())
    } else {
        Err(Error::InvalidField(tag, "field is not a string"))
    }
}

fn get_latitude(reader: &exif::Reader) -> Result<f64> {
    let mut latitude = get_degrees(reader, exif::Tag::GPSLatitude)?;
    let ref_ = get_string(reader, exif::Tag::GPSLatitudeRef)?;
    if ref_.ends_with("S") {
        latitude = -latitude;
    }
    Ok(latitude)
}

fn get_longitude(reader: &exif::Reader) -> Result<f64> {
    let mut longitude = get_degrees(reader, exif::Tag::GPSLongitude)?;
    let ref_ = get_string(reader, exif::Tag::GPSLongitudeRef)?;
    if ref_.ends_with("W") {
        longitude = -longitude;
    }
    Ok(longitude)
}

arg_enum!{
    enum Property {
        Filename,
        Path,
        Datetime,
    }
}

fn get_feature(filename: &Path, properties: &[Property]) -> Result<Feature> {
    let file = std::fs::File::open(filename)?;

    let reader = exif::Reader::new(&mut std::io::BufReader::new(&file))?;

    let latitude = get_latitude(&reader)?;
    let longitude = get_longitude(&reader)?;
    let point: geo_types::Point<f64> = (longitude, latitude).into();

    let mut props = Map::new();

    for prop in properties {
        let key = prop.to_string();
        let value = match prop {
            Property::Filename => to_value(filename.file_name().unwrap().to_string_lossy()),
            Property::Path => {
                let path = filename.canonicalize()?;
                to_value(path.to_string_lossy())
            },
            Property::Datetime => {
                let data = get_string(&reader, exif::Tag::DateTimeOriginal)?;
                to_value(data)
            }
        };
        props.insert(key, value.unwrap());
    }

    Ok(Feature {
        bbox: None,
        geometry: Some(Geometry::new(Value::from(&point))),
        id: None,
        properties: Some(props),
        foreign_members: None,
    })
}

fn main() {
    let matches = clap::App::new("plag")
        .version("0.1")
        .author("Oskari Timperi <oskari.timperi@iki.fi>")
        .about("Photo Location As GeoJSON - Extract GPS location from photos to GeoJSON")
        .arg(clap::Arg::with_name("pretty")
            .long("pretty")
            .help("Output human-readable GeoJSON"))
        .arg(clap::Arg::with_name("properties")
            .long("properties")
            .takes_value(true)
            .use_delimiter(true)
            .help("Comma-separated list of properties to set on features.")
            .possible_values(&Property::variants())
            .case_insensitive(true))
        .arg(clap::Arg::with_name("filelist")
            .long("filelist")
            .takes_value(true)
            .help("A file containing filenames")
            .long_help("A file containing filenames. There must be one filename per line."))
        .arg(clap::Arg::with_name("files")
            .required_unless("filelist")
            .multiple(true)
            .help("A list of photos"))
        .get_matches();

    let mut all_files: Vec<std::ffi::OsString> = Vec::new();

    if let Some(values) = matches.values_of_os("files") {
        all_files.extend(values.map(|s| s.into()));
    }

    if let Some(path) = matches.value_of_os("filelist") {
        let contents = std::fs::read_to_string(path);
        if contents.is_err() {
            eprintln!("error: {}: {}", path.to_string_lossy(), contents.unwrap_err());
            std::process::exit(1);
        }
        let contents = contents.unwrap();
        all_files.extend(contents.lines().map(str::trim).filter(|s| !s.is_empty()).map(|s| s.into()));
    }

    let valid_properties = values_t!(matches.values_of("properties"), Property).unwrap_or_else(|e| e.exit());

    let features: Vec<_> = all_files.into_iter()
        .filter_map(|path| {
            match get_feature(Path::new(&path), &valid_properties) {
                Ok(feature) => Some(feature),
                Err(error) => {
                    eprintln!("{}: {}", path.to_string_lossy(), error);
                    None
                }
            }
        })
        .collect();

    let collection = FeatureCollection {
        bbox: None,
        features: features,
        foreign_members: None,
    };

    let geojson = GeoJson::from(collection);

    if matches.is_present("pretty") {
        serde_json::to_writer_pretty(std::io::stdout(), &geojson).unwrap();
    } else {
        serde_json::to_writer(std::io::stdout(), &geojson).unwrap();
    }
}
