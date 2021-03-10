use px_core::model::PortInput;
use super::{
    regmatch::MatchExpr,
    regmatch::AlignedSet,
    parser::ProbeExpr,
};
use crate::error::Error;
use super::parser::{Protocol};

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    net::SocketAddr,
    num::ParseIntError
};

use regex::Regex;

#[derive(Debug)]
struct Link {
    proto: Protocol,// TCP/UDP
    payload: Vec<u8>,
    name: String,
    
    ports: Vec<PortInput>,
    exclude: Vec<PortInput>,
    tls_ports: Vec<PortInput>,

    //so here we have a flat map where we'll do a quick match on, 
    //where we get a collection of indexes matched, we'll take those 
    fallback: String,
    lookup_set: AlignedSet,
}

impl Link {
    fn matches<'a>(&'a self, input_buf: &[u8], out_buf: &mut Vec<&'a MatchExpr>) {
        self.lookup_set.match_response(input_buf, out_buf)
    }
}

pub struct ChainedProbes {
    inner: Vec<Link>,
    name_map: HashMap<String, usize>
}

/// immutable collection of probe & trigger combinations
/// once created, its contents shouldn't be modified
// [x] rarity order + load order
// [x] all enteries with fallbacks do exist, or go to Null 
impl ChainedProbes {

    #[inline]
    fn inner_new(x: ProbeExpr) -> Result<Link, Error> {
        let mut this = Link {
            proto: x.proto,
            // interpret bytes TODO
            payload: construct_payload(&x.payload)?,
            name: x.name,
            ports: x.ports,
            exclude: x.exclude,
            tls_ports: x.tls_ports,
            fallback: x.fallback.unwrap(),
            lookup_set: AlignedSet::new(&x.matches).unwrap()
        };
    
        this.tls_ports.shrink_to_fit();
        this.ports.shrink_to_fit();
        this.exclude.shrink_to_fit();
        this.name.shrink_to_fit();
        this.payload.shrink_to_fit();
        this.fallback.shrink_to_fit();
    
        Ok(this)
    }
    

    #[inline]
    fn deduplicate_probes(mut last_state: (Option<String>, Vec<ProbeExpr>), probe: ProbeExpr) -> (Option<String>, Vec<ProbeExpr>) {
        if let Some(ref last_name) = last_state.0 {
            if probe.name != *last_name {
                last_state.0 = Some(probe.name.clone());
                last_state.1.push(probe);
            }
            else { eprintln!("duplicate probe name found: {}", probe.name) }
        }
        else {
            // assume its the first iteration
            last_state.0 = Some(probe.name);
        }
        last_state
    }

    pub fn new(mut buf: Vec<ProbeExpr>, max_intensity: u8) -> Result<Self, Error> {
        // sort by name
        buf.sort_by(|a, b| a.name.partial_cmp(&b.name).unwrap());
        
        // look for duplicates, and de-duplicate
        let mut dedup = buf.drain(..)
            .fold(
                (None::<String>, Vec::new()), Self::deduplicate_probes
            ).1;
        drop(buf);

        // okay, now we need to do a fallback link check.
        // to ensure they actually go to something

        // copy all the names into this buffer
        let name_buf: Vec<String> = dedup
            .iter()
            .map(|probe| probe.name.clone())
            .collect();
        
        // use the buffer to see if anything shows up that doesn't exist
        let mut linked: Vec<_> = dedup.drain(..).map(|mut probe| {
            if let Some(fallback) = &mut probe.fallback {

                // map to no fallback if it doesn't exist
                if !name_buf.contains(&fallback.to_string()) {
                    probe.fallback = None;
                }
            }
            probe
        }).collect();
        drop(name_buf);

        // now that we're de-duped and all unknown fallbacks are set to None,
        // we will set them to NULL (the probe), but before we can do that
        // we have to find NULL (again, the probe.)

        // sort buffer by rarity, and then by load order
        linked.sort_by(|a, b| {
            // so this little condition should
            // put NULL as index 0
            if a.name == "NULL" {
                return Ordering::Greater
            }

            else if b.name == "NULL" {
                return Ordering::Less
            }

            // in all other cases, we'll sort on rarity, and then if equal, then load order
            let cmp = a.rarity.partial_cmp(&b.rarity).unwrap();
            match cmp {
                Ordering::Equal => return a.load_ord.partial_cmp(&b.load_ord).unwrap(),
                _ => return cmp
            }
        });
        
        // cut off after intensity is met
        let mut linked_probes: Vec<_> = linked.drain(..)
            .take_while(|probe| probe.rarity <= max_intensity)
            .collect();
        
        let mut name_map = HashMap::new();
        for (i, mut probe) in linked_probes.iter_mut().enumerate() {
            //reset None to NULL probe
            if let None = probe.fallback {
                probe.fallback = Some("NULL".to_string());
            }
            name_map.insert(probe.name.clone(), i).unwrap();
        }
        
        let mut links = Vec::with_capacity(linked_probes.len());
        for item in linked_probes {
            links.push(Self::inner_new(item)?)
        }

        // as far as we're concerned, 
        // this is now a flat list of probes that are ordered
        // first from rarity, then load order
        Ok(Self {
            inner: links,
            name_map
        })
    }

    fn null(&self) -> &Link {
        self.inner.get(0).unwrap()
    }

}

fn construct_payload(payload: &str) -> Result<Vec<u8>, Error> {
    lazy_static::lazy_static! {
        static ref HEX_BYTE: Regex = Regex::new("(\\x[a-hA-H0-9][a-hA-H0-9])*").unwrap();
    }

    let mut buf: Vec<u8> = Vec::new();
    
    let replacements: Vec<_> = HEX_BYTE.find_iter(payload)
        .map(|match_| (match_.start(), u8::from_str_radix(&match_.as_str()[1..3], 16).unwrap()))
        .collect();
    
    let mut replacement_idx: usize = 0;
    let mut skip: usize = 0;

    for (i, c) in payload.chars().enumerate() {
        if skip > 0 { skip -= 1; continue }

        match replacements.get(replacement_idx) {
            Some((start, byte)) => {
                //"\x00"
                if i == *start {
                    buf.push(*byte);
                    replacement_idx += 1;
                    skip=3;
                }
                else {
                    buf.push(c as u8);
                }
            },
            None => return Err(Error::ParseError(format!("expected replacement")))
        }
    }
   
    Ok(buf)
}
