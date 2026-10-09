use tokio::io::BufReader;

use super::{LineRead, read_line_capped};

const LIMIT: usize = 16;

/// Read every line from `input` through a reader that hands out at most
/// `chunk` bytes per fill, so lines straddle buffer refills.
async fn read_all(input: &[u8], chunk: usize) -> Vec<(LineRead, Vec<u8>)> {
    let mut reader = BufReader::with_capacity(chunk, input);
    let mut line = Vec::new();
    let mut reads = Vec::new();
    loop {
        let read = read_line_capped(&mut reader, &mut line, LIMIT)
            .await
            .expect("in-memory reads cannot fail");
        assert!(line.capacity() <= 2 * LIMIT, "the buffer stays bounded");
        reads.push((read, line.clone()));
        if read == LineRead::Closed {
            return reads;
        }
    }
}

#[tokio::test]
async fn lines_reassemble_across_buffer_refills() {
    for chunk in [1, 3, 64] {
        assert_eq!(
            read_all(b"{\"a\":1}\n\n{\"b\":2}\n", chunk).await,
            vec![
                (LineRead::Line, b"{\"a\":1}".to_vec()),
                (LineRead::Line, Vec::new()),
                (LineRead::Line, b"{\"b\":2}".to_vec()),
                (LineRead::Closed, Vec::new()),
            ],
            "chunk size {chunk}"
        );
    }
}

#[tokio::test]
async fn an_oversized_line_is_skipped_and_the_next_line_still_frames() {
    let at_limit = [b'a'; LIMIT];
    let over_limit = [b'b'; LIMIT + 1];
    let mut input = Vec::new();
    for line in [&at_limit[..], &over_limit[..], b"next"] {
        input.extend_from_slice(line);
        input.push(b'\n');
    }
    for chunk in [1, 5, 64] {
        assert_eq!(
            read_all(&input, chunk).await,
            vec![
                (LineRead::Line, at_limit.to_vec()),
                (LineRead::Oversized, Vec::new()),
                (LineRead::Line, b"next".to_vec()),
                (LineRead::Closed, Vec::new()),
            ],
            "chunk size {chunk}"
        );
    }
}

#[tokio::test]
async fn an_unterminated_last_line_is_still_returned() {
    assert_eq!(
        read_all(b"tail", 2).await,
        vec![
            (LineRead::Line, b"tail".to_vec()),
            (LineRead::Closed, Vec::new()),
        ]
    );
    // A peer that never sends a newline is cut off at the limit, not
    // buffered until it closes.
    assert_eq!(
        read_all(&[b'x'; 4 * LIMIT], 3).await,
        vec![
            (LineRead::Oversized, Vec::new()),
            (LineRead::Closed, Vec::new()),
        ]
    );
}
