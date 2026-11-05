use super::*;
#[test]
fn generated_flags_preserve_format_and_literals() {
    let format = Format::compile(r"FLAG\{[A-Z0-9]{8}\}").unwrap();
    let value = format.generate().unwrap();
    assert_eq!(value.len(), 14);
    assert!(value.starts_with(b"FLAG{"));
    assert!(value.ends_with(b"}"));
    assert_eq!(format.ranges(&value).len(), 1);
    for bad in ["a+", "FOO|BAR", ".*", r"[\x00-\xff]{8}", "literal"] {
        assert!(Format::compile(bad).is_err(), "{bad}");
    }
}
#[test]
fn http_chunked_replacement_preserves_every_framing_byte_at_all_splits() {
    let wire=b"POST /put HTTP/1.1\r\nHost: service\r\nTransfer-Encoding: chunked\r\n\r\n5;ext=x\r\nFLAG{\r\n9\r\n12345678}\r\n0\r\nX-Trailer: keep\r\n\r\n";
    for split in 0..=wire.len() {
        let mut framer = Framer::new(hydra_domain::ForwardFraming::Http1, false).unwrap();
        let mut frames = framer.feed(&wire[..split], false, None).unwrap();
        frames.extend(framer.feed(&wire[split..], false, None).unwrap());
        assert_eq!(frames.len(), 1);
        let mut frame = frames.remove(0);
        frame
            .rewrite(|value| {
                Ok(value
                    .windows(14)
                    .position(|x| x == b"FLAG{12345678}")
                    .map(|p| vec![(p..p + 14, b"FLAG{ABCDEFGH}".to_vec())])
                    .unwrap_or_default())
            })
            .unwrap();
        let expected = wire.windows(8).position(|x| x == b"12345678").unwrap();
        let mut wanted = wire.to_vec();
        wanted[expected..expected + 8].copy_from_slice(b"ABCDEFGH");
        assert_eq!(frame.wire, wanted);
    }
}
#[test]
fn binary_frames_and_http_head_preserve_boundaries() {
    let mut f = Framer::new(
        hydra_domain::ForwardFraming::LengthPrefixed {
            offset: 1,
            width: 2,
            little_endian: false,
            length_includes_header: false,
        },
        false,
    )
    .unwrap();
    assert!(f.feed(&[0, 0], false, None).unwrap().is_empty());
    let frames = f.feed(&[3, 0, 255, 1, 0, 0, 0], false, None).unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0].regions[0], vec![3..6]);
    let mut f = Framer::new(hydra_domain::ForwardFraming::Http1, true).unwrap();
    assert_eq!(
        f.feed(
            b"HTTP/1.1 200 OK\r\nContent-Length: 999\r\n\r\n",
            false,
            Some("HEAD")
        )
        .unwrap()
        .len(),
        1
    );
}
#[test]
fn malformed_or_partial_frames_never_silently_pass_originals() {
    let mut f = Framer::new(
        hydra_domain::ForwardFraming::Delimited {
            delimiter: "\n".into(),
        },
        false,
    )
    .unwrap();
    assert!(f.feed(b"partial FLAG", true, None).is_err());
    let mut f = Framer::new(hydra_domain::ForwardFraming::Http1, false).unwrap();
    assert!(
        f.feed(
            b"POST / HTTP/1.1\r\nContent-Encoding: gzip\r\nContent-Length: 0\r\n\r\n",
            false,
            None
        )
        .is_err()
    );
}

#[test]
fn delimiter_bytes_are_never_replacement_payload() {
    let mut framer = Framer::new(
        hydra_domain::ForwardFraming::Delimited {
            delimiter: "ENDX".into(),
        },
        false,
    )
    .unwrap();
    let mut frame = framer.feed(b"payloadENDX", false, None).unwrap().remove(0);
    frame
        .rewrite(|bytes| {
            Ok(Format::compile("END[A-Z]")?
                .ranges(bytes)
                .into_iter()
                .map(|range| (range, b"ENDY".to_vec()))
                .collect())
        })
        .unwrap();
    assert_eq!(frame.wire, b"payloadENDX");
}
