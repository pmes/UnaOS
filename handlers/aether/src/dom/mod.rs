use kuchiki::traits::*;
use kuchiki::NodeRef;

pub fn parse_html(html: &str) -> NodeRef {
    kuchiki::parse_html().one(html)
}
